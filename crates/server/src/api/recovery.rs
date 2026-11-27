//! The empty answer of a request whose work starts only once the answer is handed off:
//! a request for a reset link or a confirmation link, whose answer and its timing are to
//! be the same whatever the work finds.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use futures_util::{Stream, future::BoxFuture};

use super::no_content;
use crate::app::App;

/// The empty acknowledgement owns unstarted work until HTTP releases the response.
/// A 204 body is normally dropped without polling; a disconnected client also
/// releases it. Neither body polling nor dropping waits for the background job.
pub(super) struct RecoveryResponse {
    app: App,
    work: Option<BoxFuture<'static, ()>>,
}

impl RecoveryResponse {
    /// The empty 204 that starts `work` once HTTP releases it.
    pub(super) fn releasing(
        app: App,
        work: impl Future<Output = ()> + Send + 'static,
    ) -> axum::response::Response {
        let mut response = no_content();
        *response.body_mut() = Body::from_stream(Self {
            app,
            work: Some(Box::pin(work)),
        });
        response
    }
}

impl Stream for RecoveryResponse {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }
}

impl Drop for RecoveryResponse {
    fn drop(&mut self) {
        if let Some(work) = self.work.take() {
            self.app.spawn_background(work);
        }
    }
}
