//! Measuring the operations on the file storage into the metrics: how long each took,
//! and whether the disk or the object store failed it.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use futures_util::TryStreamExt as _;
use futures_util::stream::StreamExt as _;
use tokio::io::{AsyncRead, ReadBuf};

use super::{FileStream, StorageError};
use crate::metrics::{FilesBackend, FilesOperation, FilesOutcome, Metrics};

/// An operation being measured, from its start. Dropped before it ended, it is measured
/// as [`FilesOutcome::Abandoned`]: its request gave it up.
pub(super) struct Measuring<'m> {
    metrics: &'m Metrics,
    backend: FilesBackend,
    operation: FilesOperation,
    started: Instant,
    ended: bool,
}

impl<'m> Measuring<'m> {
    /// Starts measuring `operation` on `backend`; `None` if no metrics are kept.
    pub(super) fn start(
        metrics: &'m Metrics,
        backend: FilesBackend,
        operation: FilesOperation,
    ) -> Option<Self> {
        metrics.is_kept().then(|| Self {
            metrics,
            backend,
            operation,
            started: Instant::now(),
            ended: false,
        })
    }

    /// Measures the operation as it ended.
    pub(super) fn end(mut self, outcome: FilesOutcome) {
        self.ended = true;
        self.measure(outcome);
    }

    fn measure(&self, outcome: FilesOutcome) {
        self.metrics.files_operated(
            self.backend,
            self.operation,
            outcome,
            self.started.elapsed(),
        );
    }
}

impl Drop for Measuring<'_> {
    fn drop(&mut self) {
        if !self.ended {
            self.measure(FilesOutcome::Abandoned);
        }
    }
}

/// How an operation that answered `result` ended: a file that is not there is an
/// answer, every other error a failure.
fn outcome_of<T>(result: &Result<T, StorageError>) -> FilesOutcome {
    let Err(error) = result else {
        return FilesOutcome::Answered;
    };
    // Every error is named, so that a new one is sorted where it is made.
    match error {
        StorageError::NotFound(_) => FilesOutcome::Answered,
        // A path the server made that could leave the root is a fault of the server's.
        StorageError::InvalidPath(_)
        | StorageError::Config(_)
        | StorageError::Io(_)
        | StorageError::ObjectStore(_) => FilesOutcome::Failed,
    }
}

/// Runs `work`, which is `operation` on `backend`, and measures it into `metrics`.
pub(super) async fn measured<T>(
    metrics: &Metrics,
    backend: FilesBackend,
    operation: FilesOperation,
    work: impl Future<Output = Result<T, StorageError>>,
) -> Result<T, StorageError> {
    let Some(measuring) = Measuring::start(metrics, backend, operation) else {
        return work.await;
    };
    let result = work.await;
    measuring.end(outcome_of(&result));
    result
}

/// The input of a put, which remembers whether reading it failed. A put whose input
/// failed (the request's body went past a limit, could not be read, or its client went
/// away) is no failure of the storage's: it is measured as given up.
pub(super) struct WatchedInput<'r> {
    inner: &'r mut (dyn AsyncRead + Send + Unpin),
    failed: bool,
}

impl<'r> WatchedInput<'r> {
    pub(super) fn new(inner: &'r mut (dyn AsyncRead + Send + Unpin)) -> Self {
        Self {
            inner,
            failed: false,
        }
    }

    /// How the put of this input that answered `result` ended.
    pub(super) fn outcome<T>(&self, result: &Result<T, StorageError>) -> FilesOutcome {
        if self.failed {
            FilesOutcome::Abandoned
        } else {
            outcome_of(result)
        }
    }
}

impl AsyncRead for WatchedInput<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut *self.inner).poll_read(cx, buf);
        if matches!(polled, Poll::Ready(Err(_))) {
            self.failed = true;
        }
        polled
    }
}

/// The bytes of a get, an error among them counted as a failure of the get; the stream
/// as it is if no metrics are kept.
pub(super) fn counting_failures(
    stream: FileStream,
    metrics: &Metrics,
    backend: FilesBackend,
) -> FileStream {
    if !metrics.is_kept() {
        return stream;
    }
    let metrics = metrics.clone();
    stream
        .inspect_err(move |_| metrics.files_failed_later(backend, FilesOperation::Get))
        .boxed()
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::stream;

    use super::super::{FileBackend, LocalFiles};
    use super::*;

    fn rendered(metrics: &Metrics) -> String {
        let text = metrics
            .render()
            .expect("the metrics are kept")
            .expect("the metrics encode");
        String::from_utf8(text).expect("the text format is UTF-8")
    }

    /// The count and the failures of `operation` on the local disk.
    fn local(text: &str, operation: &str) -> (String, String) {
        let labels = format!("{{backend=\"local\",operation=\"{operation}\"}}");
        let value = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(&format!("{name}{labels} ")))
                .unwrap_or("none")
                .to_owned()
        };
        (
            value("fvoci_files_operation_duration_seconds_count"),
            value("fvoci_files_operation_failures_total"),
        )
    }

    /// A directory of its own for a test, under the system's temporary directory.
    fn directory(test: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fvoci-measure-{test}-{}",
            crate::auth::secret::new_secret()
        ))
    }

    #[tokio::test]
    async fn a_missing_file_is_an_answer_and_a_path_that_leaves_the_root_a_failure() {
        let root = directory("missing");
        let metrics = Metrics::kept().expect("the metrics register");
        let files = FileBackend::Local(LocalFiles::new(&root)).measured(metrics.clone());
        assert!(matches!(
            files.read("a/missing.txt", None).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            files.copy("a/missing.txt", "a/b.txt").await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            files.delete("../outside.txt").await,
            Err(StorageError::InvalidPath(_))
        ));
        let text = rendered(&metrics);
        assert_eq!(
            local(&text, "get"),
            ("1".to_owned(), "0".to_owned()),
            "{text}"
        );
        assert_eq!(
            local(&text, "copy"),
            ("1".to_owned(), "0".to_owned()),
            "{text}"
        );
        assert_eq!(
            local(&text, "delete"),
            ("1".to_owned(), "1".to_owned()),
            "{text}"
        );
        drop(tokio::fs::remove_dir_all(&root).await);
    }

    #[tokio::test]
    async fn a_put_whose_input_fails_is_given_up_and_no_failure() {
        let root = directory("input");
        let metrics = Metrics::kept().expect("the metrics register");
        let files = FileBackend::Local(LocalFiles::new(&root)).measured(metrics.clone());
        let mut stored: &[u8] = b"abc";
        files.write("a/b.txt", &mut stored).await.expect("stored");
        let mut broken = tokio_util::io::StreamReader::new(stream::iter([
            Ok(Bytes::from_static(b"ab")),
            Err(std::io::Error::other("the client went away")),
        ]));
        assert!(files.write("a/c.txt", &mut broken).await.is_err());
        let text = rendered(&metrics);
        assert_eq!(
            local(&text, "put"),
            ("2".to_owned(), "0".to_owned()),
            "both measured, neither failed: {text}"
        );
        drop(tokio::fs::remove_dir_all(&root).await);
    }

    #[tokio::test]
    async fn an_error_while_the_bytes_are_read_is_a_failure_of_the_get() {
        let metrics = Metrics::kept().expect("the metrics register");
        let read: FileStream = stream::iter([
            Ok(Bytes::from_static(b"ab")),
            Err(StorageError::Io(std::io::Error::other("the disk failed"))),
        ])
        .boxed();
        let chunks: Vec<_> = counting_failures(read, &metrics, FilesBackend::Local)
            .collect()
            .await;
        assert_eq!(chunks.len(), 2);
        let text = rendered(&metrics);
        assert_eq!(local(&text, "get").1, "1", "{text}");
    }

    #[tokio::test]
    async fn an_operation_its_request_gives_up_is_measured_and_no_failure() {
        let metrics = Metrics::kept().expect("the metrics register");
        let never = measured(
            &metrics,
            FilesBackend::Local,
            FilesOperation::Head,
            std::future::pending::<Result<(), StorageError>>(),
        );
        let given_up = tokio::time::timeout(std::time::Duration::from_millis(10), never).await;
        assert!(given_up.is_err(), "it never ends");
        let text = rendered(&metrics);
        assert_eq!(
            local(&text, "head"),
            ("1".to_owned(), "none".to_owned()),
            "{text}"
        );
    }

    #[tokio::test]
    async fn without_metrics_nothing_is_measured_and_the_stream_is_as_it_was() {
        let metrics = Metrics::none();
        let result = measured(&metrics, FilesBackend::S3, FilesOperation::Put, async {
            Err::<(), _>(StorageError::Config("x".to_owned()))
        })
        .await;
        assert!(result.is_err());
        metrics.files_known(FilesBackend::S3);
        metrics.files_failed_later(FilesBackend::S3, FilesOperation::Get);
        assert!(metrics.render().is_none(), "nothing is kept");
        let read: FileStream = stream::iter([Ok(Bytes::from_static(b"ab"))]).boxed();
        let chunks: Vec<_> = counting_failures(read, &metrics, FilesBackend::S3)
            .collect()
            .await;
        assert_eq!(chunks.len(), 1);
    }
}
