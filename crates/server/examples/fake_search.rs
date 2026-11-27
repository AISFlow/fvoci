//! The tests' search server, as a program: for the end-to-end tests of the web app
//! against the real server.
//!
//! ```text
//! cargo run -p fvoci-server --example fake_search -- <port> [elasticsearch|opensearch]
//! ```
//!
//! It listens on `127.0.0.1:<port>` over plain HTTP: the server is pointed at it with
//! `search_url` (`FVOCI_SEARCH_URL=http://127.0.0.1:<port>`). It keeps the documents
//! the server sends in memory and finds them by their letters, knowing no language. A
//! test drives it by the routes under `/_fake` (`tests/api/fake_search.rs`): it can be
//! made to refuse everything, to refuse writes, or to answer every search with a
//! document a test plants. It prints `fake-search: <address>` once it listens, and
//! runs until it is stopped.

#[path = "../tests/api/fake_search.rs"]
mod fake_search;

use std::io::Write as _;

use fake_search::{FakeSearch, Flavor};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let port: u16 = arguments.next().expect("a port").parse()?;
    let flavor = match arguments.next().as_deref() {
        Some("elasticsearch") => Flavor::Elasticsearch,
        Some("opensearch") | None => Flavor::OpenSearch,
        Some(other) => anyhow::bail!("{other:?} is neither elasticsearch nor opensearch"),
    };
    let search = FakeSearch::start_on(port, flavor).await;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "fake-search: http://{}", search.address)?;
    stdout.flush()?;
    std::future::pending::<()>().await;
    Ok(())
}
