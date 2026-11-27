//! Connections the server can break.
//!
//! A response whose client takes nothing is not even polled again: the socket takes no
//! more, so nothing in the response can end it. The connection's IO is wrapped so that a
//! [`Breaker`] handed to the request (in [`Connection`], its connect info) can make every
//! read and write of it fail from then on, which ends the connection.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use futures_util::task::AtomicWaker;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::request_log::Peer;

/// Where a connection comes from, and how to break it: the connect info of a request.
#[derive(Debug, Clone)]
pub struct Connection {
    /// Where the connection comes from.
    pub peer: Peer,
    /// Breaks the connection; none on the administration socket.
    pub breaker: Option<Breaker>,
}

/// Breaks one connection.
#[derive(Debug, Clone, Default)]
pub struct Breaker(Arc<Switch>);

#[derive(Debug, Default)]
struct Switch {
    broken: AtomicBool,
    /// The task that last polled the connection's IO.
    waker: AtomicWaker,
}

impl Breaker {
    /// Breaks the connection: its reads and writes fail from now on, also those that
    /// wait now.
    pub fn cut(&self) {
        self.0.broken.store(true, Ordering::Release);
        self.0.waker.wake();
    }

    /// Whether the connection is broken.
    pub fn is_cut(&self) -> bool {
        self.0.broken.load(Ordering::Acquire)
    }

    /// Fails if the connection is broken; else remembers the task to wake when it is.
    fn check(&self, cx: &Context<'_>) -> io::Result<()> {
        self.0.waker.register(cx.waker());
        if self.0.broken.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the server broke the connection",
            ))
        } else {
            Ok(())
        }
    }
}

/// The IO of a connection that a [`Breaker`] can break.
#[derive(Debug)]
pub struct Breakable<Io> {
    io: Io,
    breaker: Breaker,
}

impl<Io> Breakable<Io> {
    /// Wraps `io`, with a new breaker.
    pub fn new(io: Io) -> Self {
        Self {
            io,
            breaker: Breaker::default(),
        }
    }

    /// The breaker of this connection.
    pub fn breaker(&self) -> &Breaker {
        &self.breaker
    }
}

impl<Io: AsyncRead + Unpin> AsyncRead for Breakable<Io> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.breaker.check(cx)?;
        Pin::new(&mut this.io).poll_read(cx, buf)
    }
}

impl<Io: AsyncWrite + Unpin> AsyncWrite for Breakable<Io> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.breaker.check(cx)?;
        Pin::new(&mut this.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.breaker.check(cx)?;
        Pin::new(&mut this.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.breaker.check(cx)?;
        Pin::new(&mut this.io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Shutting down a broken connection is what is wanted of it.
        Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

/// A listener whose connections can be broken.
#[derive(Debug)]
pub struct BreakableListener<L>(pub L);

impl<L> Listener for BreakableListener<L>
where
    L: Listener,
    L::Io: Unpin,
{
    type Io = Breakable<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (io, address) = self.0.accept().await;
        (Breakable::new(io), address)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

impl Connected<IncomingStream<'_, BreakableListener<tokio::net::TcpListener>>> for Connection {
    fn connect_info(
        stream: IncomingStream<'_, BreakableListener<tokio::net::TcpListener>>,
    ) -> Self {
        Self {
            peer: Peer::Tcp(*stream.remote_addr()),
            breaker: Some(stream.io().breaker().clone()),
        }
    }
}

impl Connected<IncomingStream<'_, BreakableListener<crate::tls::TlsListener>>> for Connection {
    fn connect_info(
        stream: IncomingStream<'_, BreakableListener<crate::tls::TlsListener>>,
    ) -> Self {
        Self {
            peer: Peer::Tcp(*stream.remote_addr()),
            breaker: Some(stream.io().breaker().clone()),
        }
    }
}

#[cfg(unix)]
impl Connected<IncomingStream<'_, tokio::net::UnixListener>> for Connection {
    fn connect_info(_stream: IncomingStream<'_, tokio::net::UnixListener>) -> Self {
        Self {
            peer: Peer::Unix,
            breaker: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    /// A write that waits for a peer that reads nothing fails once the connection is
    /// broken, and so does every read and write after.
    #[tokio::test]
    async fn a_broken_connection_fails_also_what_waits() {
        let (near, mut far) = tokio::io::duplex(16);
        let mut near = Breakable::new(near);
        let breaker = near.breaker().clone();
        let waiting = tokio::spawn(async move {
            let written = near.write_all(&[0; 64]).await;
            (written, near)
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the write waits for the peer");

        breaker.cut();
        let (written, mut near) = tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("the waiting write ends")
            .expect("the task ran");
        let error = written.expect_err("the write fails");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        assert!(near.write_all(b"x").await.is_err(), "so does the next");
        let mut buffer = [0; 4];
        far.write_all(b"abcd").await.expect("the peer writes");
        assert!(near.read(&mut buffer).await.is_err(), "and every read");
    }

    /// A request served on a breakable listener has its connection's breaker in its
    /// connect info, and cutting it ends the connection before any answer goes out.
    #[tokio::test]
    async fn a_request_can_break_its_connection() {
        use axum::extract::ConnectInfo;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("its address");
        let router = axum::Router::new().route(
            "/",
            axum::routing::get(
                |ConnectInfo(connection): ConnectInfo<Connection>| async move {
                    match &connection.breaker {
                        Some(breaker) => {
                            breaker.cut();
                            "cut"
                        }
                        None => "no breaker",
                    }
                },
            ),
        );
        let service = router.into_make_service_with_connect_info::<Connection>();
        let server = tokio::spawn(axum::serve(BreakableListener(listener), service).into_future());

        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connected");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("asked");
        let mut answer = Vec::new();
        let ended = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stream.read_to_end(&mut answer),
        )
        .await
        .expect("the connection ends");
        server.abort();
        assert!(
            ended.is_err() || answer.is_empty(),
            "no answer: {}",
            String::from_utf8_lossy(&answer)
        );
    }
}
