//! Sending an export, and taking it back from a client that stopped reading.
//!
//! An export holds its place among the exports that run ([`Slot`]) and its temporary
//! file until the last piece of the archive is read, or the client goes away: the limit
//! on exports is a limit on the disk they take at once. Both are given back before the
//! last piece is handed over, so that a client that has the whole archive finds the
//! file gone and the place free, and its next export is not refused for this one. A
//! client that stops reading would hold both as long as its connection lasts; the
//! body's stream is then not even polled again (the socket takes nothing more), so no
//! timer inside it would fire. A watch beside it sees
//! how far the sending got, and when nothing went out for the stall time it takes the
//! file and the place away (the file is closed and removed, then the place is free) and
//! breaks the connection, with what the handler gave it for that; a stream polled after
//! that ends with an error.
//!
//! What went out is what hyper took from the stream, which it takes as the kernel's send
//! buffer and its own (a few MiB together) make room: a client must take about that much
//! within the stall time not to be given up.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::Stream;
use tokio::io::AsyncReadExt as _;
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::Slot;
use super::export::ExportedArchive;

/// How much of the archive one poll of the stream sends.
const PIECE_BYTES: usize = 64 * 1024;

/// What a download holds while it is read: the archive and the place, or nothing once
/// they are given back. Dropped, it removes the archive's file and then gives the place
/// back, in that order: the place stands for the disk the file takes.
#[derive(Debug)]
struct Held(Option<(ExportedArchive, Slot)>);

impl Drop for Held {
    fn drop(&mut self) {
        let Some((archive, slot)) = self.0.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(release(archive, slot));
        } else {
            // Without a runtime the archive's drop removes its file as it can; then the
            // place goes.
            drop(archive);
            drop(slot);
        }
    }
}

impl Held {
    /// Gives back what the download held, and waits for it. In a task of its own: the
    /// file goes before the place also when whoever waits is dropped meanwhile.
    async fn give_back(mut self) {
        let Some((archive, slot)) = self.0.take() else {
            return;
        };
        if let Err(error) = tokio::spawn(release(archive, slot)).await {
            tracing::warn!(%error, "an export's file was not seen to be removed");
        }
    }
}

/// Removes the archive's file, and then gives the place back.
async fn release(archive: ExportedArchive, slot: Slot) {
    archive.remove().await;
    drop(slot);
}

/// How far a download is.
#[derive(Debug)]
enum Progress {
    /// Pieces are still to be read: the archive and the place are held.
    Reading(Held),
    /// The last piece was read, or reading failed: nothing is held.
    Read,
    /// The watch took the archive and the place away from a client that took nothing.
    GivenUp,
}

/// A download, shared by its stream and its watch.
#[derive(Debug)]
struct Sending {
    /// Tokio's lock: it is held across the read of a piece, so that the watch never
    /// takes the file in the middle of one.
    progress: Mutex<Progress>,
    /// How many bytes went out so far.
    sent: AtomicU64,
}

/// The body of an export's download: the archive in pieces. A watch takes the archive and
/// `slot` away, and calls `cut`, which is to break the connection the download goes out
/// on, when no piece went out for `stall`.
///
/// `cut` is called at most once, from the watch's task, and not at all when the download
/// is sent or dropped. It must not wait: the download is locked while it runs.
pub(crate) fn download(
    archive: ExportedArchive,
    slot: Slot,
    stall: Duration,
    cut: impl FnOnce() + Send + 'static,
) -> impl Stream<Item = io::Result<Bytes>> + Send + 'static {
    let sending = Arc::new(Sending {
        progress: Mutex::new(Progress::Reading(Held(Some((archive, slot))))),
        sent: AtomicU64::new(0),
    });
    tokio::spawn(watch(Arc::downgrade(&sending), stall, cut));
    futures_util::stream::unfold(Some(sending), |sending| async move {
        let sending = sending?;
        let piece = next_piece(&sending).await;
        match piece {
            Ok(Some(bytes)) => Some((Ok(bytes), Some(sending))),
            Ok(None) => None,
            Err(error) => Some((Err(error), None)),
        }
    })
}

/// The next piece of the archive; `None` at its end. What the download held is given
/// back, and that waited for, before the last piece is returned: whoever has the whole
/// archive finds nothing of it left.
async fn next_piece(sending: &Sending) -> io::Result<Option<Bytes>> {
    let mut progress = sending.progress.lock().await;
    let archive = match &mut *progress {
        Progress::Reading(Held(Some((archive, _)))) => archive,
        Progress::Reading(Held(None)) | Progress::Read => return Ok(None),
        Progress::GivenUp => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the client took nothing for too long; the export was given up",
            ));
        }
    };
    let mut buffer = BytesMut::with_capacity(PIECE_BYTES);
    let outcome = archive.file.read_buf(&mut buffer).await;
    let size = archive.size;
    let (piece, ended) = match outcome {
        Ok(0) => (Ok(None), true),
        Ok(bytes) => {
            let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
            let sent = sending
                .sent
                .fetch_add(bytes, Ordering::Relaxed)
                .saturating_add(bytes);
            (Ok(Some(buffer.freeze())), sent >= size)
        }
        Err(error) => (Err(error), true),
    };
    if !ended {
        return piece;
    }
    // The lock is for the read: the watch ends on what it finds from here on, and
    // need not wait for the file to go.
    let done = std::mem::replace(&mut *progress, Progress::Read);
    drop(progress);
    if let Progress::Reading(held) = done {
        held.give_back().await;
    }
    piece
}

/// Watches a download: when no byte went out for `stall`, takes what it holds away and
/// breaks the connection, with `cut`. Ends when the download is sent, taken away or
/// dropped.
async fn watch(sending: Weak<Sending>, stall: Duration, cut: impl FnOnce()) {
    let tick = (stall / 4).max(Duration::from_millis(50));
    let mut last_sent = 0;
    let mut last_progress = Instant::now();
    loop {
        tokio::time::sleep(tick).await;
        let Some(sending) = sending.upgrade() else {
            return;
        };
        let mut progress = sending.progress.lock().await;
        if !matches!(*progress, Progress::Reading(_)) {
            return;
        }
        // Read under the lock: a piece read while the watch waited for it counts.
        let sent = sending.sent.load(Ordering::Relaxed);
        if sent != last_sent {
            last_sent = sent;
            last_progress = Instant::now();
        }
        if last_progress.elapsed() >= stall {
            *progress = Progress::GivenUp;
            cut();
            tracing::info!(
                sent,
                "an export's download took nothing for too long and was given up"
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use futures_util::StreamExt as _;

    use super::*;
    use crate::app::doomed::Doomed;
    use crate::app::slots::Slots;

    /// An archive of four pieces in a file of its own; returns it and where it is.
    async fn archive(name: &str) -> (ExportedArchive, std::path::PathBuf) {
        archive_of(name, 4 * PIECE_BYTES, 4 * PIECE_BYTES).await
    }

    /// An archive said to be of `size` bytes, in a file of its own that has `bytes`.
    async fn archive_of(
        name: &str,
        bytes: usize,
        size: usize,
    ) -> (ExportedArchive, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("download-{}-{name}", std::process::id()));
        std::fs::write(&path, vec![7; bytes]).expect("the archive is written");
        let file = tokio::fs::File::open(&path).await.expect("opened");
        let size = u64::try_from(size).expect("a size");
        (
            ExportedArchive::new(file, size, Doomed::file(path.clone())),
            path,
        )
    }

    /// A connection as the download knows it: whether it was cut, and what cuts it.
    fn connection() -> (Arc<AtomicBool>, impl FnOnce() + Send + 'static) {
        let is_cut = Arc::new(AtomicBool::new(false));
        let cut = {
            let is_cut = Arc::clone(&is_cut);
            move || is_cut.store(true, Ordering::Release)
        };
        (is_cut, cut)
    }

    /// Waits, on the paused clock, until `done`.
    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..1000 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what}");
    }

    /// A client that takes a piece and then pauses, for less than the stall time each
    /// time but longer than it in all, gets the whole archive; the place is held until
    /// the last piece, with which the file is gone and the place given back. On the paused clock, so
    /// that how busy the machine is does not matter.
    #[tokio::test(start_paused = true)]
    async fn a_download_taken_slowly_is_not_given_up() {
        let slots = Slots::new(1);
        let (archive, path) = archive("slow").await;
        let size = archive.size;
        let slot = slots.take("user").expect("a place");
        let (is_cut, cut) = connection();
        let stall = Duration::from_secs(1);
        let pieces = download(archive, slot, stall, cut);
        tokio::pin!(pieces);

        let started = Instant::now();
        let mut received = 0;
        while let Some(piece) = pieces.next().await {
            received += piece.expect("not given up").len();
            if u64::try_from(received).ok() < Some(size) {
                assert!(
                    slots.take("other").is_err(),
                    "the place is held while it is sent"
                );
            } else {
                // The last piece: whoever has it finds nothing held any more, at once.
                assert!(!path.exists(), "the file went with the last piece");
                assert!(
                    slots.take("other").is_ok(),
                    "the place is free with the last piece"
                );
            }
            tokio::time::sleep(stall * 6 / 10).await;
        }
        assert_eq!(
            u64::try_from(received).ok(),
            Some(size),
            "the whole archive"
        );
        assert!(
            started.elapsed() > stall * 2,
            "it took longer than the stall time"
        );
        assert!(!is_cut.load(Ordering::Acquire), "the connection stays");
        until("the place is given back", || slots.take("user").is_ok()).await;
        assert!(!path.exists(), "the file went before the place");
    }

    /// The file goes before the place: polled step by step, the release never has the
    /// place free while the file is there.
    #[tokio::test]
    async fn the_file_goes_before_the_place() {
        let slots = Slots::new(1);
        let (archive, path) = archive("released").await;
        let slot = slots.take("user").expect("a place");
        let releasing = release(archive, slot);
        tokio::pin!(releasing);
        loop {
            let done = futures_util::poll!(releasing.as_mut()).is_ready();
            let free = slots.take("user").is_ok();
            assert!(
                !free || !path.exists(),
                "the place is free while the file is there"
            );
            if done {
                assert!(free && !path.exists(), "both are given back");
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    /// A file that ends before the size it was said to have, and one of no bytes at
    /// all: the stream ends where the file does, and by then nothing is held.
    #[tokio::test]
    async fn a_download_that_ends_short_holds_nothing_at_its_end() {
        for (name, bytes) in [("short", PIECE_BYTES), ("empty", 0)] {
            let slots = Slots::new(1);
            let (archive, path) = archive_of(name, bytes, 4 * PIECE_BYTES).await;
            let slot = slots.take("user").expect("a place");
            let (is_cut, cut) = connection();
            let pieces = download(archive, slot, Duration::from_secs(60), cut);
            tokio::pin!(pieces);
            let mut received = 0;
            while let Some(piece) = pieces.next().await {
                received += piece.expect("read").len();
                assert!(path.exists(), "{name}: held while more may come");
            }
            assert_eq!(received, bytes, "{name}: what the file has");
            assert!(!path.exists(), "{name}: the file went with the end");
            assert!(slots.take("user").is_ok(), "{name}: and the place");
            assert!(!is_cut.load(Ordering::Acquire), "{name}: nothing was cut");
        }
    }

    /// A download dropped while pieces remain, and one dropped while its last piece
    /// waits for the file to go: the file goes before the place either way.
    #[tokio::test]
    async fn a_download_that_is_dropped_gives_the_file_back_before_the_place() {
        for (name, taken) in [("dropped", 1), ("dropped-last", 3)] {
            let slots = Slots::new(1);
            let (archive, path) = archive(name).await;
            let slot = slots.take("user").expect("a place");
            let (_is_cut, cut) = connection();
            let mut pieces = Box::pin(download(archive, slot, Duration::from_secs(60), cut));
            for _ in 0..taken {
                pieces.next().await.expect("a piece").expect("read");
            }
            if taken == 3 {
                // The last piece is read, and waits for what was held to be given back.
                assert!(
                    futures_util::poll!(pieces.next()).is_pending(),
                    "{name}: not handed over before the file is gone"
                );
            }
            drop(pieces);
            for _ in 0..10_000 {
                let free = slots.take("user").is_ok();
                assert!(
                    !free || !path.exists(),
                    "{name}: the place is free while the file is there"
                );
                if free {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(!path.exists(), "{name}: the file is gone");
        }
    }

    /// A download that takes nothing for the stall time is given up: the file goes, then
    /// the place, the connection is cut, and the stream, polled again, ends with an error.
    /// On the paused clock the moment is exact: the watch looks every quarter of the stall
    /// time, sees the piece at its first look, and gives up at its look a stall time
    /// after that.
    #[tokio::test(start_paused = true)]
    async fn a_download_that_takes_nothing_is_given_up() {
        let slots = Slots::new(1);
        let (archive, path) = archive("stalled").await;
        let slot = slots.take("user").expect("a place");
        let (is_cut, cut) = connection();
        let stall = Duration::from_secs(1);
        let started = Instant::now();
        let pieces = download(archive, slot, stall, cut);
        tokio::pin!(pieces);
        pieces.next().await.expect("a piece").expect("read");

        tokio::time::sleep_until(started + stall * 5 / 4 - Duration::from_millis(10)).await;
        assert!(slots.take("other").is_err(), "held until the watch's look");
        assert!(!is_cut.load(Ordering::Acquire));
        tokio::time::sleep_until(started + stall * 5 / 4 + Duration::from_millis(10)).await;
        assert!(is_cut.load(Ordering::Acquire), "cut at the watch's look");
        until("the place is given back", || slots.take("user").is_ok()).await;
        assert!(!path.exists(), "the file went before the place");
        assert!(is_cut.load(Ordering::Acquire), "the connection is cut");
        let error = pieces.next().await.expect("an end").expect_err("given up");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(pieces.next().await.is_none(), "and nothing after");
    }
}
