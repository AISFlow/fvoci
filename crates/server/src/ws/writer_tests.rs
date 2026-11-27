//! Revoking access while a sink holds a frame must not flush that frame on close.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use super::*;

#[derive(Default)]
struct Transport {
    writable: AtomicBool,
    buffered: tokio::sync::Notify,
    waker: Mutex<Option<Waker>>,
    emitted: Mutex<Vec<Message>>,
}

struct Backpressured {
    transport: Arc<Transport>,
    pending: Vec<Message>,
}

impl Sink<Message> for Backpressured {
    type Error = Infallible;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        self.pending.push(message);
        self.transport.buffered.notify_one();
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if !self.transport.writable.load(Ordering::SeqCst) {
            *self.transport.waker.lock().expect("test waker lock") = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let pending = std::mem::take(&mut self.pending);
        self.transport
            .emitted
            .lock()
            .expect("test output lock")
            .extend(pending);
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

#[tokio::test]
async fn ending_a_session_drops_a_buffered_frame_without_flushing_it_on_close() {
    let transport = Arc::new(Transport::default());
    let sink = Backpressured {
        transport: Arc::clone(&transport),
        pending: Vec::new(),
    };
    let closed = CancellationToken::new();
    let stopped = closed.clone();
    let task = tokio::spawn(async move {
        let reason = OnceLock::new();
        reason
            .set(Disconnect::SessionEnded)
            .expect("initial reason");
        write(
            sink,
            futures_util::stream::iter([Message::Text("private contents".into())]),
            &stopped,
            &reason,
        )
        .await;
    });
    tokio::time::timeout(Duration::from_secs(5), transport.buffered.notified())
        .await
        .expect("the sink buffers the private frame");
    closed.cancel();
    transport.writable.store(true, Ordering::SeqCst);
    if let Some(waker) = transport.waker.lock().expect("test waker lock").take() {
        waker.wake();
    }
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("writer stops")
        .expect("writer returns");
    assert!(
        transport
            .emitted
            .lock()
            .expect("test output lock")
            .is_empty(),
        "neither a close nor its flush may release revoked content"
    );
}
