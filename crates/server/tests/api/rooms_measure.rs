//! Twenty connections in one room, in turn and all writing at once for a while: how
//! long an update takes to be answered and to reach everyone, and what the server and
//! its worker hold meanwhile.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::Barrier;

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{
    Incoming, PATIENCE, Peer, UPDATE, card_holds, give, paragraphs, table, take_varuint,
};

/// The resident memory of a process, in KiB, as `/proc` has it; `None` where there
/// is no `/proc`.
fn resident_kib(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn millis(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

/// The middle, the ninety-fifth of a hundred and the longest of `times`.
fn spread(times: &mut [Duration]) -> String {
    assert!(!times.is_empty(), "something was measured");
    times.sort_unstable();
    let at = |hundredth: usize| times[(times.len() - 1) * hundredth / 100];
    format!(
        "median {} ms, p95 {} ms, max {} ms",
        millis(at(50)),
        millis(at(95)),
        millis(at(100))
    )
}

#[expect(clippy::print_stderr, reason = "the measurements are for the report")]
fn report(line: &str) {
    let profile = if cfg!(debug_assertions) {
        "debug build"
    } else {
        "release build"
    };
    eprintln!("measured ({profile}): {line}");
}

/// Twenty connections in the room of one card write in turn, and then all at once:
/// every one holds the same document in the end, and so does the card. Prints how
/// long an update took until its sender was answered and until the last of the other
/// nineteen had it, and the memory of the server's process (which is this test's)
/// and of its worker.
#[tokio::test]
async fn twenty_connections_edit_one_body() {
    const PEERS: usize = 20;
    const ROUNDS: usize = 5;
    let th = Setup::native()
        // All twenty are one user's.
        .configure(|config| config.collab.collab_connections_per_user = 20)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let before = resident_kib(std::process::id());
    let mut peers = Vec::new();
    for client in 0..PEERS {
        let mut peer = Peer::connect(&th.client, 100 + u64::try_from(client).expect("few")).await;
        peer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        peers.push(peer);
    }

    // In turn: each update is answered and has reached everyone before the next.
    let (mut answered, mut reached) = (Vec::new(), Vec::new());
    for round in 0..ROUNDS {
        for writer in 0..PEERS {
            let began = Instant::now();
            peers[writer].write(&format!("{round}.{writer}")).await;
            let ack = peers[writer].text("docAck").await;
            assert_eq!(ack["epoch"], json!(1), "{ack}");
            answered.push(began.elapsed());
            for (reader, peer) in peers.iter_mut().enumerate() {
                if reader == writer {
                    continue;
                }
                match peer.incoming(PATIENCE).await {
                    Incoming::Frame(frame) => {
                        assert_eq!(frame.sync, UPDATE, "{frame:?}");
                        peer.editor().receive(&frame.payload);
                    }
                    other => panic!("the update reaches everyone: {other:?}"),
                }
            }
            reached.push(began.elapsed());
        }
    }
    report(&format!(
        "{PEERS} connections in one room, {} updates in turn: answered (docAck) {}; \
         at the last of the other {} {}",
        answered.len(),
        spread(&mut answered),
        PEERS - 1,
        spread(&mut reached),
    ));

    // All at once: each writes before it has the others'.
    let began = Instant::now();
    for (writer, peer) in peers.iter_mut().enumerate() {
        peer.write(&format!("at once {writer}")).await;
    }
    for peer in &mut peers {
        peer.text("docAck").await;
    }
    let all_answered = began.elapsed();
    for peer in &mut peers {
        peer.settle().await;
    }
    let body = peers[0].body().expect("a body");
    assert_eq!(
        peers[0].lines().len(),
        1 + ROUNDS * PEERS + PEERS,
        "every update is in it"
    );
    for peer in &peers {
        assert_eq!(peer.body().as_ref(), Some(&body), "all hold the same");
    }
    let worker = th.app.collab().worker_pids().await[0];
    report(&format!(
        "{PEERS} updates at once: all answered after {} ms; resident memory of the \
         server's process (with the test and its {PEERS} editors) {} KiB before the \
         connections and {} KiB after, of the worker {} KiB",
        millis(all_answered),
        before.map_or_else(|| "?".to_owned(), |kib| kib.to_string()),
        resident_kib(std::process::id()).map_or_else(|| "?".to_owned(), |kib| kib.to_string()),
        worker
            .and_then(resident_kib)
            .map_or_else(|| "?".to_owned(), |kib| kib.to_string()),
    ));
    drop(peers);
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// How many users the twenty connections of the room under load are: two tabs each,
/// within `collab_connections_per_user` as it is by default.
const USERS: usize = 10;
/// How long everyone writes at once.
const WRITING: Duration = Duration::from_secs(10);
/// How often each connection writes: someone typing steadily, as the web app gathers
/// what is typed into frames (at most 33 a second). Twenty of them are forty updates a
/// second for the room, which takes them one after another: a debug build on SQLite
/// took some sixty a second, in turn, on the four cores of the machine it was first
/// measured on, and eight a second each (160 for the room) fell behind there.
const EVERY: Duration = Duration::from_millis(500);

/// The client of the editor whose update `update` is; `None` for one that holds no
/// items, only what it deletes (the server's repair). An update of one paragraph by
/// one editor holds the items of that one.
fn writer_of(update: &[u8]) -> Option<u64> {
    let mut bytes = update;
    let clients = take_varuint(&mut bytes);
    if clients == 0 {
        return None;
    }
    assert_eq!(clients, 1, "an update of one writer: {update:?}");
    let _structs = take_varuint(&mut bytes);
    Some(take_varuint(&mut bytes))
}

/// When the writers of the room under load sent each of their updates, by writer and
/// in order.
#[derive(Default)]
struct Sent(Mutex<HashMap<u64, Vec<Instant>>>);

impl Sent {
    fn put(&self, writer: u64, at: Instant) {
        let mut sent = self.0.lock().expect("lock poisoned");
        sent.entry(writer).or_default().push(at);
    }

    /// When the `nth` update of `writer` was sent.
    fn when(&self, writer: u64, nth: usize) -> Option<Instant> {
        let sent = self.0.lock().expect("lock poisoned");
        sent.get(&writer)?.get(nth).copied()
    }

    fn counts(&self) -> HashMap<u64, usize> {
        let sent = self.0.lock().expect("lock poisoned");
        sent.iter()
            .map(|(writer, at)| (*writer, at.len()))
            .collect()
    }
}

/// What one connection of the room under load saw: how long each update of the others
/// took to reach it, how long each of its own took to be answered, and how many of
/// each writer's it took.
#[derive(Default)]
struct Seen {
    reached: Vec<Duration>,
    answered: Vec<Duration>,
    taken: HashMap<u64, usize>,
    /// When its updates that are not answered yet were sent, in order.
    waiting: VecDeque<Instant>,
    /// When the last update of the others came.
    last: Option<Instant>,
}

impl Seen {
    /// Takes what came into the peer's document, or as the answer to its update.
    fn take(&mut self, peer: &mut Peer, sent: &Sent, incoming: Incoming) {
        match incoming {
            Incoming::Frame(frame) => {
                assert_eq!((frame.doc, frame.sync), (peer.doc, UPDATE), "{frame:?}");
                if let Some(writer) = writer_of(&frame.payload) {
                    let nth = self.taken.entry(writer).or_default();
                    let at = sent
                        .when(writer, *nth)
                        .unwrap_or_else(|| panic!("{writer} sent an update {nth}"));
                    self.reached.push(at.elapsed());
                    *nth += 1;
                    self.last = Some(Instant::now());
                }
                peer.editor().receive(&frame.payload);
            }
            Incoming::Text(message) if message["type"] == json!("docAck") => {
                let at = self.waiting.pop_front().expect("an update is waiting");
                self.answered.push(at.elapsed());
            }
            Incoming::Nothing => {}
            other => panic!(
                "{}: only updates and answers come: {other:?}",
                peer.client()
            ),
        }
    }

    /// Whether it took every update the others sent, and was answered all of its own.
    fn has_all(&self, me: u64, sent: &HashMap<u64, usize>) -> bool {
        self.waiting.is_empty()
            && sent
                .iter()
                .all(|(writer, count)| *writer == me || self.taken.get(writer) == Some(count))
    }
}

/// One connection of the room under load: writes every [`EVERY`] until `stop`, taking
/// in what comes meanwhile, and then reads until it holds all that the others sent.
async fn write_and_read(
    mut peer: Peer,
    sent: Arc<Sent>,
    stop: Instant,
    stopped: Arc<Barrier>,
) -> (Peer, Seen) {
    let me = peer.client();
    let mut seen = Seen::default();
    let mut next = Instant::now();
    let mut n = 0;
    while Instant::now() < stop {
        if Instant::now() >= next {
            let at = Instant::now();
            sent.put(me, at);
            seen.waiting.push_back(at);
            peer.write(&format!("{me}.{n}")).await;
            n += 1;
            next += EVERY;
        }
        let wait = next
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        let incoming = peer.incoming(wait).await;
        seen.take(&mut peer, &sent, incoming);
    }
    // Everyone stopped writing: what each sent is all there is to have. One that
    // failed does not come: the others do not wait for it for ever.
    tokio::time::timeout(PATIENCE, stopped.wait())
        .await
        .expect("everyone stopped writing");
    let all = sent.counts();
    // A quarter of a second for each update, which the room takes one after another,
    // past the minute: a machine four times slower than the first one measured on.
    let written = u32::try_from(all.values().sum::<usize>()).expect("a count");
    let deadline = Instant::now() + PATIENCE * 6 + Duration::from_millis(250) * written;
    while !seen.has_all(me, &all) {
        assert!(
            Instant::now() < deadline,
            "{me} holds all that was sent: {:?} of {all:?}; {} answers wait; the last update \
             came {:?} after the writing stopped",
            seen.taken,
            seen.waiting.len(),
            seen.last.map(|last| last.saturating_duration_since(stop)),
        );
        let incoming = peer.incoming(PATIENCE).await;
        seen.take(&mut peer, &sent, incoming);
    }
    (peer, seen)
}

/// Twenty connections of ten users, two tabs each, as the server is configured by
/// default, write into the room of one card all at once for ten seconds, twice a
/// second each: every one holds the same document in the end, and so does the card.
/// Prints how long an update took to reach each of the others and to be answered, how
/// many bytes the worker holds of the document, and the memory of the worker and of
/// the server's process (which is this test's, with its twenty editors, and with any
/// other test that runs meanwhile: `--test-threads=1` for a figure of its own). The
/// server runs on four threads, as it does when it is deployed on such a machine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_connections_write_at_once_for_a_while() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut clients = vec![th.client.clone(), th.client2.clone()];
    for user in 2..USERS {
        clients.push(th.add_user(&format!("writer{user}")).await);
    }
    for client in &clients[1..] {
        give(&th, &t.board_id, &client.me().await.id, "editor").await;
    }
    let before = resident_kib(std::process::id());
    let mut peers = Vec::new();
    for (n, client) in clients
        .iter()
        .flat_map(|client| [client, client])
        .enumerate()
    {
        let mut peer = Peer::connect(client, 300 + u64::try_from(n).expect("few")).await;
        peer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        peers.push(peer);
    }
    let joined = resident_kib(std::process::id());

    let sent = Arc::new(Sent::default());
    let stopped = Arc::new(Barrier::new(peers.len()));
    let began = Instant::now();
    let stop = began + WRITING;
    let writing: Vec<_> = peers
        .into_iter()
        .map(|peer| {
            tokio::spawn(write_and_read(
                peer,
                Arc::clone(&sent),
                stop,
                Arc::clone(&stopped),
            ))
        })
        .collect();
    let (mut reached, mut answered, mut peers) = (Vec::new(), Vec::new(), Vec::new());
    let mut last = began;
    // The first that fails is told at once, whichever it is.
    let done = futures_util::future::try_join_all(writing)
        .await
        .expect("a connection of the room writes and reads");
    for (peer, seen) in done {
        reached.extend(seen.reached);
        answered.extend(seen.answered);
        last = last.max(seen.last.unwrap_or(began));
        peers.push(peer);
    }
    let took = last - began;
    let held = th
        .app
        .collab()
        .held_state_bytes(&th.app, &t.card_id)
        .await
        .expect("the document is open");
    let after = resident_kib(std::process::id());
    let worker = th.app.collab().worker_pids().await[0];

    let written: usize = sent.counts().values().sum();
    let body = peers[0].body().expect("a body");
    assert_eq!(peers[0].lines().len(), 1 + written, "every update is in it");
    for peer in &peers {
        assert_eq!(peer.body().as_ref(), Some(&body), "all hold the same");
    }
    let kib = |kib: Option<u64>| kib.map_or_else(|| "?".to_owned(), |kib| kib.to_string());
    report(&format!(
        "{} connections of {USERS} users in one room, all writing every {} ms for {} s: \
         {written} updates, everywhere {} ms after the first was sent; at each of the \
         other {} {}; answered (docAck) {}; the \
         worker holds {held} bytes of the document's state and its resident memory is {} \
         KiB; the whole test process (the server, the test's editors, and any other test \
         running meanwhile) {} KiB before the connections, {} KiB with them in the \
         room, {} KiB after the writing",
        peers.len(),
        EVERY.as_millis(),
        WRITING.as_secs(),
        took.as_millis(),
        peers.len() - 1,
        spread(&mut reached),
        spread(&mut answered),
        kib(worker.and_then(resident_kib)),
        kib(before),
        kib(joined),
        kib(after),
    ));
    drop(peers);
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}
