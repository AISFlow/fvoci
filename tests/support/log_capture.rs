//! WARN-and-above log capture for parallel `#[tokio::test]`s in one binary.
//!
//! A per-thread `tracing::subscriber::set_default` capture can miss events.
//! tracing-core caches each callsite's interest process-wide when the callsite
//! is first reached, and while exactly one dispatcher is registered it asks
//! only the default dispatcher of the thread that reached it
//! (tracing-core 0.1.36 `callsite.rs`: `Rebuilder::JustOne` calls
//! `dispatcher::get_default`). A test thread without a capture of its own that
//! reaches a callsite first, while another test's capture is the only one
//! registered, caches `Interest::never` for everyone, and the capturing thread
//! then never sees that event.
//!
//! Here one global subscriber, installed once, formats every WARN-and-above
//! event, so each callsite's interest is the same whichever thread computes
//! it. Its writer appends the line to the capture active on the emitting
//! thread, or prints it to the test output (libtest's per-test capture) when
//! that thread has none. The binary must not install another subscriber:
//! [`install`] panics if a global one (or a `log` logger) exists, and a
//! `set_default` anywhere in the binary brings the per-thread hazard back.

use std::cell::RefCell;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, Once};

/// Lines written on one thread while its [`CaptureGuard`] lives.
#[derive(Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    pub fn lines(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn lines_with(&self, needle: &str) -> Vec<String> {
        let mut lines = self.lines();
        lines.retain(|line| line.contains(needle));
        lines
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Captured>> = const { RefCell::new(None) };
}

/// Ends the capture on drop, restoring the thread's previous one (if any).
#[must_use = "dropping the guard ends the capture"]
pub struct CaptureGuard(Option<Captured>);

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        let _ = ACTIVE.try_with(|active| *active.borrow_mut() = previous);
    }
}

/// Installs the global router (and, as `try_init` does, the `log` bridge).
/// Idempotent: harnesses and tests may all call it.
pub fn install() {
    static ROUTER: Once = Once::new();
    ROUTER.call_once(|| {
        tracing_subscriber::fmt()
            .with_writer(|| Routed)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .try_init()
            .expect("the log capture router is this binary's only subscriber");
    });
}

/// Captures the WARN-and-above lines emitted on this thread until the guard
/// drops. A `#[tokio::test]` runs a current-thread runtime, so the tasks it
/// spawns log here too.
pub fn capture_warnings() -> (Captured, CaptureGuard) {
    install();
    let captured = Captured::default();
    let previous = ACTIVE.with(|active| active.replace(Some(captured.clone())));
    (captured, CaptureGuard(previous))
}

/// The router's writer: the emitting thread's active capture, else the test
/// output.
struct Routed;

impl Write for Routed {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let active = ACTIVE
            .try_with(|active| active.borrow().clone())
            .ok()
            .flatten();
        match active {
            Some(captured) => captured.0.lock().unwrap().extend_from_slice(buf),
            None => tracing_subscriber::fmt::TestWriter::new().write_all(buf)?,
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
