#![cfg(feature = "worker")]
//! Test-only allocation/range oracle for the native archive witness charges.
//! A counting global allocator measures cumulative and peak live heap while
//! the public retained-witness APIs run on one ordinary near-cap document,
//! and the same in-order IdSet inserts are replayed to record the real range
//! behaviour. Diagnostic evidence only; it changes no product limit or path.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, Ordering::Relaxed};

use collab_engine::Limits;
use yrs::retained::{self, InputEntry, VisitLimits, WitnessCost, WitnessLimits};
use yrs::updates::decoder::Decode;
use yrs::{IdSet, Transact, Update, ID};

/// Thread-scoped counting allocator: only the measuring thread's requests
/// are attributed to a phase; any other thread's allocation or free while a
/// phase is measured is counted as foreign. Realloc counts its full new
/// request; the momentary old+new overlap inside a moving realloc is not
/// visible to the peak (documented limitation, not physical RSS).
struct Counting;
static CURRENT: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static FOREIGN: AtomicUsize = AtomicUsize::new(0);
static MEASURING: AtomicBool = AtomicBool::new(false);
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}
fn active() -> bool {
    ACTIVE.try_with(Cell::get).unwrap_or(false)
}
/// Per-size request histogram of the measuring thread: exact sizes up to
/// SMALL bytes in a fixed array; larger sizes in fixed open-addressed slots.
/// Nothing here allocates (static atomics only).
const SMALL: usize = 4096;
static SIZES: [AtomicUsize; SMALL + 1] = [const { AtomicUsize::new(0) }; SMALL + 1];
static LARGE_SIZE: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static LARGE_COUNT: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
static LARGE_OVERFLOW: AtomicUsize = AtomicUsize::new(0);
fn histogram(n: usize) {
    if n <= SMALL {
        SIZES[n].fetch_add(1, Relaxed);
        return;
    }
    for i in 0..64 {
        let slot = (n.wrapping_mul(0x9e37_79b9) + i) % 64;
        match LARGE_SIZE[slot].compare_exchange(0, n, Relaxed, Relaxed) {
            Ok(_) => {
                LARGE_COUNT[slot].fetch_add(1, Relaxed);
                return;
            }
            Err(current) if current == n => {
                LARGE_COUNT[slot].fetch_add(1, Relaxed);
                return;
            }
            Err(_) => {}
        }
    }
    LARGE_OVERFLOW.fetch_add(1, Relaxed);
}
fn reset_histogram() {
    for c in &SIZES {
        c.store(0, Relaxed);
    }
    for i in 0..64 {
        LARGE_SIZE[i].store(0, Relaxed);
        LARGE_COUNT[i].store(0, Relaxed);
    }
    LARGE_OVERFLOW.store(0, Relaxed);
}
/// (size, count) of every requested layout since the last reset.
fn histogram_rows() -> Vec<(usize, usize)> {
    let mut rows: Vec<(usize, usize)> = SIZES
        .iter()
        .enumerate()
        .filter_map(|(size, c)| {
            let n = c.load(Relaxed);
            (n > 0).then_some((size, n))
        })
        .collect();
    for i in 0..64 {
        let n = LARGE_COUNT[i].load(Relaxed);
        if n > 0 {
            rows.push((LARGE_SIZE[i].load(Relaxed), n));
        }
    }
    rows.sort_unstable();
    rows
}
fn requested(n: usize) {
    if active() {
        histogram(n);
        TOTAL.fetch_add(n, Relaxed);
        COUNT.fetch_add(1, Relaxed);
        let now = CURRENT.fetch_add(n as isize, Relaxed) + n as isize;
        PEAK.fetch_max(now, Relaxed);
    } else if MEASURING.load(Relaxed) {
        FOREIGN.fetch_add(1, Relaxed);
    }
}
fn released(n: usize) {
    if active() {
        CURRENT.fetch_sub(n as isize, Relaxed);
    } else if MEASURING.load(Relaxed) {
        FOREIGN.fetch_add(1, Relaxed);
    }
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            requested(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            requested(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        released(layout.size());
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new) };
        if !q.is_null() {
            released(layout.size());
            requested(new);
        }
        q
    }
}
#[global_allocator]
static ALLOC: Counting = Counting;

#[derive(Debug, Default, Clone, Copy)]
struct Heap {
    /// Sum of successful requested layouts (full size for realloc).
    cumulative: usize,
    allocations: usize,
    /// Highest net live bytes of this thread since the phase start.
    peak_live_delta: isize,
    /// Net live bytes at the phase end (frees of older memory go negative).
    retained_delta: isize,
    /// Allocator events from other threads during the phase.
    foreign: usize,
}
fn measure<T>(f: impl FnOnce() -> T) -> (T, Heap) {
    CURRENT.store(0, Relaxed);
    PEAK.store(0, Relaxed);
    let (total, count, foreign) = (
        TOTAL.load(Relaxed),
        COUNT.load(Relaxed),
        FOREIGN.load(Relaxed),
    );
    MEASURING.store(true, Relaxed);
    ACTIVE.with(|a| a.set(true));
    let value = f();
    ACTIVE.with(|a| a.set(false));
    MEASURING.store(false, Relaxed);
    let heap = Heap {
        cumulative: TOTAL.load(Relaxed) - total,
        allocations: COUNT.load(Relaxed) - count,
        peak_live_delta: PEAK.load(Relaxed),
        retained_delta: CURRENT.load(Relaxed),
        foreign: FOREIGN.load(Relaxed) - foreign,
    };
    (value, heap)
}

#[derive(Default)]
struct Charged {
    calls: usize,
    work: usize,
    inspected: usize,
    owned: usize,
    shapes: Option<HashMap<(usize, usize, usize), usize>>,
}
impl Charged {
    fn shaped() -> Self {
        Self {
            shapes: Some(HashMap::new()),
            ..Self::default()
        }
    }
    fn add(&mut self, c: WitnessCost) -> bool {
        self.calls += 1;
        self.work += c.work;
        self.inspected += c.inspected_bytes;
        self.owned += c.owned_bytes;
        if let Some(shapes) = &mut self.shapes {
            *shapes
                .entry((c.work, c.inspected_bytes, c.owned_bytes))
                .or_default() += 1;
        }
        true
    }
}

/// The product's witness/visit limits for `Limits::default()`.
fn witness_limits() -> WitnessLimits {
    let l = Limits::default();
    WitnessLimits {
        max_blocks: l.max_project_nodes as usize,
        max_roots: l.max_project_depth as usize,
        max_table_capacity: l.max_project_nodes as usize,
        max_depth: l.max_project_depth as usize,
        max_native_bytes: l.max_output_bytes as usize,
    }
}

#[test]
#[ignore = "diagnostic allocation/range oracle; run explicitly with --ignored --nocapture"]
fn native_archive_witness_allocation_and_range_oracle() {
    let paragraphs: Vec<serde_json::Value> = (0..850)
        .map(|i| serde_json::json!({"type":"paragraph","content":[{"type":"text","text":format!("{i:04}{}", "a".repeat(996))}]}))
        .collect();
    let body = serde_json::json!({"type":"doc","content":paragraphs});
    let bytes = collab_engine::seed::tiptap_to_yjs_update(&body, &Limits::default()).unwrap();
    println!("oracle corpus: update {} bytes", bytes.len());

    // Instrument controls: an allocation-free closure, one known allocation
    // and its free, and an allocation followed by a full realloc.
    let (_, quiet) = measure(|| (0..1000u64).sum::<u64>());
    assert_eq!(
        (quiet.cumulative, quiet.allocations, quiet.foreign),
        (0, 0, 0),
        "{quiet:?}"
    );
    let (boxed, one) = measure(|| Box::new([7u8; 4096]));
    assert_eq!(
        (one.cumulative, one.allocations, one.retained_delta),
        (4096, 1, 4096),
        "{one:?}"
    );
    let ((), freed) = measure(|| drop(boxed));
    assert_eq!(
        (freed.cumulative, freed.retained_delta),
        (0, -4096),
        "{freed:?}"
    );
    let (grown, realloc) = measure(|| {
        let mut v: Vec<u64> = Vec::with_capacity(16);
        v.reserve_exact(48);
        v
    });
    assert_eq!(
        (
            realloc.cumulative,
            realloc.allocations,
            realloc.retained_delta
        ),
        (16 * 8 + 48 * 8, 2, 48 * 8),
        "{realloc:?}"
    );
    drop(grown);
    println!("oracle controls: quiet {quiet:?}; box {one:?}; free {freed:?}; realloc {realloc:?}");

    // Real in-order IdSet insert behaviour for the corpus items, per insert.
    let update = Update::decode_v1(&bytes).unwrap();
    let mut ids = Vec::new();
    let l = Limits::default();
    retained::visit_input(
        &update,
        VisitLimits {
            max_blocks: l.max_project_nodes as usize,
            max_roots: l.max_project_depth as usize,
            max_table_capacity: l.max_project_nodes as usize,
        },
        |e| {
            if let InputEntry::Item {
                id,
                native_clock_len,
                ..
            } = e
            {
                ids.push((id, native_clock_len));
            }
            ControlFlow::<()>::Continue(())
        },
    )
    .unwrap();
    let mut set = IdSet::default();
    let (mut tail, mut general, mut pushes, mut grew_allocs, mut grew_bytes) = (0, 0, 0, 0, 0);
    let mut max_len = 0usize;
    for (id, len) in &ids {
        let before = set.get(&id.client).map_or(0, |r| r.len());
        let last_start = set
            .get(&id.client)
            .and_then(|r| r.iter().last().map(|(r, _)| r.start));
        if last_start.is_none_or(|start| id.clock >= start) {
            tail += 1;
        } else {
            general += 1;
        }
        let ((), heap) = measure(|| set.insert(ID::new(id.client, id.clock), *len));
        let after = set.get(&id.client).map_or(0, |r| r.len());
        if after > before {
            pushes += 1;
        }
        if heap.cumulative > 0 {
            grew_allocs += 1;
            grew_bytes += heap.cumulative;
        }
        max_len = max_len.max(after);
    }
    println!(
        "oracle idset replay: inserts {} tail-path {tail} general-path {general} range-pushes {pushes} max-ranges {max_len} inserts-that-allocated {grew_allocs} bytes-allocated {grew_bytes} clients {}",
        ids.len(),
        set.len()
    );

    // Owned input witness capture: charged versus real heap.
    let mut capture = Charged::default();
    reset_histogram();
    let (witness, heap) = measure(|| {
        retained::capture_owned_input(&update, witness_limits(), &mut |c| capture.add(c))
    });
    let witness = witness.unwrap();
    println!(
        "oracle capture_owned_input: charged work {} inspected {} owned {} calls {}; heap {heap:?}",
        capture.work, capture.inspected, capture.owned, capture.calls
    );
    let rows = histogram_rows();
    assert_eq!(LARGE_OVERFLOW.load(Relaxed), 0);
    assert_eq!(
        rows.iter().map(|(size, n)| size * n).sum::<usize>(),
        heap.cumulative,
        "histogram covers every request"
    );
    println!("oracle capture histogram (size, count): {rows:?}");
    // Charge shapes of the same capture, unmeasured.
    let mut shaped = Charged::shaped();
    retained::capture_owned_input(&update, witness_limits(), &mut |c| shaped.add(c)).unwrap();
    let mut shapes: Vec<_> = shaped.shapes.unwrap().into_iter().collect();
    shapes.sort_unstable();
    println!("oracle capture charge shapes ((work, inspected, owned), count): {shapes:?}");
    // Contract: the owned charge is an upper bound of the cumulative bytes the
    // allocator handed out during the phase (which also bounds its peak).
    assert_eq!(heap.foreign, 0, "{heap:?}");
    assert!(
        capture.owned >= heap.cumulative,
        "capture owned charge {} below cumulative allocation {}",
        capture.owned,
        heap.cumulative
    );

    // Witness verification against the applied store: charged versus real heap.
    let doc = collab_engine::engine::new_doc();
    doc.transact_mut()
        .apply_update(Update::decode_v1(&bytes).unwrap())
        .unwrap();
    let mut verify = Charged::default();
    // The caller-side transaction and witness vector exist before the phase.
    let txn = doc.transact();
    let witnesses = vec![witness];
    let (proof, heap) = measure(|| {
        retained::verify_owned_witnesses(&txn, witnesses, witness_limits(), &mut |c| verify.add(c))
    });
    drop(txn);
    let stats = proof.unwrap();
    println!(
        "oracle verify_owned_witnesses: charged work {} inspected {} owned {} calls {}; heap {heap:?}; stats {stats:?}",
        verify.work, verify.inspected, verify.owned, verify.calls
    );
    assert_eq!(heap.foreign, 0, "{heap:?}");
    assert!(
        verify.owned >= heap.cumulative,
        "verify owned charge {} below cumulative allocation {}",
        verify.owned,
        heap.cumulative
    );
    // Unmeasured pass: the same charges grouped by shape.
    let witness = retained::capture_owned_input(&update, witness_limits(), &mut |_| true).unwrap();
    let mut shaped = Charged::shaped();
    retained::verify_owned_witnesses(&doc.transact(), vec![witness], witness_limits(), &mut |c| {
        shaped.add(c)
    })
    .unwrap();
    assert_eq!((shaped.work, shaped.owned), (verify.work, verify.owned));
    let mut top: Vec<_> = shaped.shapes.unwrap().into_iter().collect();
    top.sort_by_key(|((_, _, o), n)| std::cmp::Reverse(o * n));
    println!(
        "oracle verify shapes by owned: {:?}",
        top.into_iter().take(8).collect::<Vec<_>>()
    );
}
