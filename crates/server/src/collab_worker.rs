//! The worker process: the server's own executable, started by the server in a mode
//! of its own, that holds the Y documents of the bodies being edited and is the only
//! process that hands the Y library bytes from anybody.
//!
//! The Y library has ended the process over a damaged update before. The server keeps
//! that out of its own process: what it asks of a document crosses a pipe to the
//! worker as a frame ([`frame`]) that carries a request and comes back with its
//! answer ([`wire`]), and if the worker dies, is killed or stops answering, the server
//! starts another and opens the documents again from what it stored.
//!
//! The worker reads its standard input and writes its standard output, and names
//! nothing but the documents of `fvoci_collab` and the frames: the layer it is in may
//! name no other layer and, of what is in no layer, only what is listed for it
//! (`xtask/src/layers.rs`): no file, socket, database or process.

pub mod frame;
mod serve;
pub mod wire;

pub use serve::serve;

/// The first argument that starts the executable as the worker.
pub const WORKER_ARGUMENT: &str = "collab-worker";

/// Runs the executable as the worker if its first argument is [`WORKER_ARGUMENT`]:
/// `None` if it is not, else how the worker ended ([`run`]).
#[must_use]
pub fn run_if_started_as_worker() -> Option<Result<(), frame::FrameError>> {
    let argument = std::env::args_os().nth(1)?;
    (argument == WORKER_ARGUMENT).then(run)
}

/// Runs the worker on the standard input and output. Ends when the server closes the
/// input; fails if a frame is broken, which the server sees as the worker's death.
///
/// # Errors
///
/// As [`serve`].
pub fn run() -> Result<(), frame::FrameError> {
    let mut input = std::io::BufReader::new(std::io::stdin().lock());
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    serve(&mut input, &mut output)
}
