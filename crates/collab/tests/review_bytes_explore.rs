//! The search for updates the server does not survive, run over a row of seeds.
//!
//! Without settings it runs the seeds 1 to 400, a few seconds. To search further:
//! `FVOCI_EXPLORE_SECONDS` (how long), `FVOCI_EXPLORE_FROM` (the first seed),
//! `FVOCI_EXPLORE_THREADS` (how many at once), `FVOCI_EXPLORE_TRACE=1` (name each seed
//! before it runs, to find the one that ends the process).

mod support;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use support::explore::{Failure, Slowest, note_panics, session};

fn setting(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

/// The updates of a session.
const STEPS: usize = 14;

#[test]
fn no_update_made_at_random_panics_or_leaves_what_the_server_relies_on_broken() {
    let seconds = setting("FVOCI_EXPLORE_SECONDS");
    let from = setting("FVOCI_EXPLORE_FROM").unwrap_or(1);
    let threads = setting("FVOCI_EXPLORE_THREADS").unwrap_or(4);
    let trace = setting("FVOCI_EXPLORE_TRACE") == Some(1);
    let until = seconds.map_or(from + 400, |_| u64::MAX);
    let deadline = seconds.map(|seconds| Instant::now() + Duration::from_secs(seconds));
    let next = AtomicU64::new(from);
    // The first failure of each kind, by where it panicked and what it said.
    let failures: Mutex<BTreeMap<String, (usize, Failure)>> = Mutex::new(BTreeMap::new());
    let slowest: Mutex<Slowest> = Mutex::new(Slowest::default());
    note_panics();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let seed = next.fetch_add(1, Ordering::Relaxed);
                    if seed >= until || deadline.is_some_and(|deadline| Instant::now() > deadline) {
                        break;
                    }
                    if trace {
                        println!("seed {seed}");
                    }
                    let (failure, slow) = session(seed, STEPS);
                    if let Some(failure) = failure {
                        let mut failures = failures.lock().expect("the failures");
                        failures
                            .entry(failure.what.clone())
                            .and_modify(|(count, _)| *count += 1)
                            .or_insert((1, failure));
                    }
                    let mut slowest = slowest.lock().expect("the slowest");
                    if slow.took > slowest.took {
                        *slowest = slow;
                    }
                }
            });
        }
    });
    let _ = std::panic::take_hook();
    let seeds = next.load(Ordering::Relaxed).min(until) - from;
    let slowest = slowest.into_inner().expect("the slowest");
    println!("{seeds} seeds from {from}; the slowest update: {slowest:?}");
    let failures = failures.into_inner().expect("the failures");
    for (what, (count, first)) in &failures {
        println!(
            "{count} times, first in seed {} at update {}: {what}\n  before: {}\n  update: {}",
            first.seed, first.step, first.before, first.update
        );
    }
    assert!(
        failures.is_empty(),
        "{} kinds of failure in {seeds} seeds from {from}",
        failures.len()
    );
}
