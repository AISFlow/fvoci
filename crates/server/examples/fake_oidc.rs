//! The fake `OpenID Connect` provider of the server's tests, as a program: for the
//! end-to-end tests of the web app against the real server.
//!
//! ```text
//! cargo run -p fvoci-server --example fake_oidc -- <port> <certificate file>
//! ```
//!
//! It listens on `127.0.0.1:<port>` over TLS with a certificate it makes, which it
//! writes, in PEM, to the certificate file: the server is to trust it
//! (`FVOCI_OIDC_EXTRA_CA_FILE`), and the browser to take it. Its issuer is
//! `https://127.0.0.1:<port>`, its client `fvoci-test-client` with either secret in
//! `tests/api/fake_oidc.rs`. Everybody who asks is signed in as one person,
//! `oidc-person` (`oidc-person@example.test`), until a test names another
//! (`POST /person` with `subject`, `email` and, if it has them, `preferredUsername` and
//! `name`).
//! It prints `fake-oidc: <issuer>` once it
//! listens, and runs until it is stopped.

#[path = "../tests/api/fake_oidc.rs"]
mod fake_oidc;

use std::io::Write as _;

use fake_oidc::{FakeProvider, Person};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let port: u16 = arguments
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or(0);
    let certificate_file = arguments
        .next()
        .unwrap_or_else(|| "fake-oidc.pem".to_owned());

    let provider = FakeProvider::start_on(port).await;
    let mut person = Person::new("oidc-person", "oidc-person@example.test");
    person.preferred_username = Some("oidc-person".to_owned());
    person.name = Some("OIDC Person".to_owned());
    provider.sign_in_as(person);
    std::fs::write(&certificate_file, &provider.certificate_pem)?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "fake-oidc: {}", provider.issuer)?;
    stdout.flush()?;
    std::future::pending::<()>().await;
    Ok(())
}
