//! Updates after which the Y library, collecting what was deleted, left a change that
//! pointed at memory it had freed: the next encoding of the document read it, and the
//! process ended (SIGSEGV) or the state came out different each time. The documents of
//! this crate are made without garbage collection, which these hold to.
//!
//! In a file of its own: if it comes back, it ends this test binary and no other.

mod support;

use support::explore::session;

/// The session in which the search first saw it (seed 268019): at its sixth update
/// the document the update was given for encoded otherwise than before. (The smallest
/// document the review cut from it read freed memory only now and then, so it is not
/// a test.)
#[test]
fn the_session_that_first_read_freed_memory_keeps_its_documents() {
    let (failure, _) = session(268_019, 14);
    assert!(failure.is_none(), "{failure:?}");
}
