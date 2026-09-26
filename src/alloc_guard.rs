//! Records that an allocation failed, so an rlimited child can tell an
//! address-space ceiling from a parser bug.
//!
//! Rust's own collections abort on allocation failure (SIGABRT, which the
//! parent classifies as a resource limit). Some libraries instead see the null
//! pointer and panic: `zlib-rs` (flate2's backend once `docx-rs` enables
//! `zip/deflate`) asserts in `InflateStream::new`. Without this record such a
//! panic reads as "the bytes broke the parser". `main` installs
//! [`RecordingAlloc`] as the global allocator; the office child's panic hook
//! aborts when [`allocation_failed`] is set.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};

static FAILED: AtomicBool = AtomicBool::new(false);

/// The system allocator, remembering whether any request returned null.
pub struct RecordingAlloc;

fn record(ptr: *mut u8) -> *mut u8 {
    if ptr.is_null() {
        FAILED.store(true, Ordering::Relaxed);
    }
    ptr
}

// SAFETY: every call forwards to `System` unchanged; only a null result is noted.
unsafe impl GlobalAlloc for RecordingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(unsafe { System.realloc(ptr, layout, new_size) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Whether an allocation in this process has returned null.
pub fn allocation_failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

/// Panic hook for rlimited children: a panic after a failed allocation is the
/// memory ceiling, so abort like Rust's own out-of-memory path; any other
/// panic keeps the default report and exit code 101.
pub fn abort_panics_after_allocation_failure() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if allocation_failed() {
            eprintln!("allocation failed under the address-space limit: {info}");
            std::process::abort();
        }
        default(info);
    }));
}
