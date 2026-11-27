//! A loopback SMTP sink shared by integration tests and the live-browser example.
#![allow(
    dead_code,
    reason = "the example and individual tests use different controls"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mail_parser::MessageParser;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Received {
    pub(crate) recipient: String,
    pub(crate) message: String,
    pub(crate) decoded: String,
}

#[derive(Default)]
struct Inbox {
    messages: Mutex<Vec<Received>>,
    changed: Notify,
    failures: AtomicUsize,
    /// Messages answered with a permanent refusal, before any `failures`.
    rejections: AtomicUsize,
    /// Messages whose sender is answered `530` (authentication required).
    unauthenticated: AtomicUsize,
    attempts: AtomicUsize,
    delay: Mutex<Duration>,
    /// Messages to recipients ending so are answered only once this is `None` again.
    held: watch::Sender<Option<String>>,
}

pub(crate) struct FakeSmtp {
    pub(crate) port: u16,
    inbox: Arc<Inbox>,
    accept: JoinHandle<()>,
    clients: TaskTracker,
}

impl FakeSmtp {
    pub(crate) async fn start_on(port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("SMTP listener");
        let port = listener.local_addr().expect("SMTP address").port();
        let inbox = Arc::new(Inbox::default());
        let accepting = Arc::clone(&inbox);
        let clients = TaskTracker::new();
        let tracking = clients.clone();
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let inbox = Arc::clone(&accepting);
                tracking.spawn(async move {
                    let _ = serve(stream, inbox).await;
                });
            }
        });
        Self {
            port,
            inbox,
            accept,
            clients,
        }
    }

    pub(crate) fn settings(&self) -> fvoci_server::config::smtp::SmtpSettings {
        fvoci_server::config::smtp::SmtpSettings {
            smtp_host: "127.0.0.1".to_owned(),
            smtp_port: self.port,
            smtp_tls: fvoci_server::config::smtp::SmtpTls::Unencrypted,
            mail_from: "sender@example.test".to_owned(),
            // The pace has tests of its own; these send without waiting for it.
            mail_per_second: 1000,
            ..Default::default()
        }
    }

    pub(crate) fn fail_next(&self, count: usize) {
        self.inbox.failures.store(count, Ordering::SeqCst);
    }
    /// Refuses the next `count` messages for good (`550`).
    pub(crate) fn reject_next(&self, count: usize) {
        self.inbox.rejections.store(count, Ordering::SeqCst);
    }
    /// Answers the sender of the next `count` messages with `530`, as a relay that
    /// wants credentials the server does not give: the whole way is closed.
    pub(crate) fn require_credentials_next(&self, count: usize) {
        self.inbox.unauthenticated.store(count, Ordering::SeqCst);
    }
    pub(crate) fn attempts(&self) -> usize {
        self.inbox.attempts.load(Ordering::SeqCst)
    }
    pub(crate) async fn delay(&self, delay: Duration) {
        *self.inbox.delay.lock().await = delay;
    }
    /// Leaves every message to a recipient ending with `suffix` unanswered after its
    /// data until [`FakeSmtp::release`], keeping its connection busy.
    pub(crate) fn hold(&self, suffix: &str) {
        self.inbox.held.send_replace(Some(suffix.to_owned()));
    }
    pub(crate) fn release(&self) {
        self.inbox.held.send_replace(None);
    }
    /// Waits until `count` messages had their data sent, delivered or not.
    pub(crate) async fn attempted(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while self.attempts() < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("SMTP attempts arrive");
    }
    pub(crate) async fn messages(&self) -> Vec<Received> {
        self.inbox.messages.lock().await.clone()
    }

    pub(crate) async fn received(&self, count: usize) -> Vec<Received> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let changed = self.inbox.changed.notified();
                let messages = self.messages().await;
                if messages.len() >= count {
                    return messages;
                }
                changed.await;
            }
        })
        .await
        .expect("SMTP messages arrive")
    }

    pub(crate) async fn stop(self) {
        self.accept.abort();
        let _ = self.accept.await;
        self.clients.close();
        self.clients.wait().await;
    }
}

async fn serve(stream: TcpStream, inbox: Arc<Inbox>) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    writer.write_all(b"220 fake-smtp ESMTP\r\n").await?;
    let mut recipient = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let command = line.to_ascii_uppercase();
        let response: &[u8] = if command.starts_with("EHLO") {
            b"250-fake-smtp\r\n250 8BITMIME\r\n"
        } else if command.starts_with("MAIL FROM:")
            && inbox
                .unauthenticated
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            inbox.attempts.fetch_add(1, Ordering::SeqCst);
            b"530 5.7.0 Authentication required\r\n"
        } else if command.starts_with("RCPT TO:") {
            recipient = line.trim()[8..].trim().trim_matches(['<', '>']).to_owned();
            b"250 OK\r\n"
        } else if command.starts_with("DATA") {
            writer.write_all(b"354 Send message\r\n").await?;
            let mut message = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await? == 0 {
                    return Ok(());
                }
                if line == ".\r\n" {
                    break;
                }
                message.push_str(if line.starts_with("..") {
                    &line[1..]
                } else {
                    &line
                });
            }
            inbox.attempts.fetch_add(1, Ordering::SeqCst);
            let delay = *inbox.delay.lock().await;
            tokio::time::sleep(delay).await;
            let _ = inbox
                .held
                .subscribe()
                .wait_for(|held| {
                    held.as_ref()
                        .is_none_or(|suffix| !recipient.ends_with(suffix.as_str()))
                })
                .await;
            let take = |count: &AtomicUsize| {
                count
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
            };
            if take(&inbox.rejections) {
                b"550 No such mailbox here\r\n"
            } else if take(&inbox.failures) {
                b"451 Try later\r\n"
            } else {
                inbox.messages.lock().await.push(Received {
                    recipient: recipient.clone(),
                    decoded: decoded(&message),
                    message,
                });
                inbox.changed.notify_waiters();
                b"250 Accepted\r\n"
            }
        } else if command.starts_with("QUIT") {
            writer.write_all(b"221 Bye\r\n").await?;
            return Ok(());
        } else {
            b"250 OK\r\n"
        };
        writer.write_all(response).await?;
    }
}

/// Decodes MIME with a parser independent of the production message builder.
fn decoded(message: &str) -> String {
    let parsed = MessageParser::default()
        .parse(message.as_bytes())
        .expect("valid MIME");
    format!(
        "{}\n{}",
        parsed.body_text(0).unwrap_or_default(),
        parsed.body_html(0).unwrap_or_default()
    )
}
