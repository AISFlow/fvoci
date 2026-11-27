//! The collab worker as the tests start it: what the server's executable is when it
//! is started as a worker, and nothing else.
//!
//! It is built only with the feature `test-util`, which gives the worker the request
//! of the tests (`Request::FailNextSettle`). The tests do not start their workers from
//! the server's executable: its file under `target/` is whichever build was made last,
//! and one made without the feature while the tests run (`cargo build`, `cargo run`)
//! ends at the first such request.

use anyhow::anyhow;

fn main() -> anyhow::Result<()> {
    fvoci_server::collab::run_as_worker()
        .unwrap_or_else(|| Err(anyhow!("started without the argument of a worker")))
}
