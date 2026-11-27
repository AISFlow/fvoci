//! The tests' SMTP sink, with a loopback-only inbox for live browser tests.

#[path = "../tests/api/fake_smtp.rs"]
mod fake_smtp;

use axum::{Json, Router, routing::get};
use std::io::Write as _;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let smtp_port = args.next().expect("SMTP port").parse()?;
    let inbox_port: u16 = args.next().expect("inbox HTTP port").parse()?;
    let smtp = Arc::new(fake_smtp::FakeSmtp::start_on(smtp_port).await);
    let inbox = Router::new().route(
        "/messages",
        get(move || {
            let smtp = Arc::clone(&smtp);
            async move { Json(smtp.messages().await) }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", inbox_port)).await?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "fake-smtp: {smtp_port}, inbox: {inbox_port}")?;
    stdout.flush()?;
    axum::serve(listener, inbox).await?;
    Ok(())
}
