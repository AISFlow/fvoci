//! The metrics of the file storage (`file_storage`): how long each operation on the local
//! disk or the object store took, and how many failed, by operation.

use std::time::Duration;

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// The file storage an operation was made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilesBackend {
    /// A directory on the local disk.
    Local,
    /// An S3-compatible object store.
    S3,
}

impl FilesBackend {
    /// The value of the `backend` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

/// An operation on the file storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilesOperation {
    /// Storing a file in one go, until all of it is stored: also the time the bytes
    /// take to come.
    Put,
    /// Opening a file, or a range of it, to read: until its bytes can be read.
    Get,
    /// Asking for the size of a file, or whether it is there.
    Head,
    Copy,
    Delete,
    Rename,
    /// Starting an upload in parts on the object store.
    MultipartCreate,
    /// Sending one part of it.
    MultipartPart,
    /// Completing it from its parts.
    MultipartComplete,
    /// Throwing it away with its parts.
    MultipartAbort,
}

impl FilesOperation {
    /// The operations of every backend.
    const ON_EVERY_BACKEND: [Self; 6] = [
        Self::Put,
        Self::Get,
        Self::Head,
        Self::Copy,
        Self::Delete,
        Self::Rename,
    ];

    /// The steps of an upload in parts, on the object store only.
    const MULTIPART: [Self; 4] = [
        Self::MultipartCreate,
        Self::MultipartPart,
        Self::MultipartComplete,
        Self::MultipartAbort,
    ];

    /// The value of the `operation` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Get => "get",
            Self::Head => "head",
            Self::Copy => "copy",
            Self::Delete => "delete",
            Self::Rename => "rename",
            Self::MultipartCreate => "multipart_create",
            Self::MultipartPart => "multipart_part",
            Self::MultipartComplete => "multipart_complete",
            Self::MultipartAbort => "multipart_abort",
        }
    }
}

/// Whether an operation on the file storage got an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilesOutcome {
    /// It did, whatever the answer was: done, or no such file.
    Answered,
    /// It failed: the disk or the object store gave an error, or the path the server
    /// made could not be used.
    Failed,
    /// It ended without an answer of the storage's: it was given up with its task (a
    /// shutdown), or what a put was to store stopped coming or was refused (a limit, a
    /// client gone).
    Abandoned,
}

/// The upper bounds of the buckets of the durations, in seconds: from a look at the
/// local disk to a large file stored in one go.
const BUCKETS: [f64; 14] = [
    0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0, 60.0,
];

/// The metrics of the file storage.
#[derive(Debug)]
pub(super) struct FilesMetrics {
    operations: HistogramVec,
    failures: IntCounterVec,
}

impl FilesMetrics {
    /// Makes the metrics and registers them in `registry`.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let operations = HistogramVec::new(
            HistogramOpts::new(
                "files_operation_duration_seconds",
                "How long operations on the file storage took, also those given up with \
                 their task or whose bytes stopped coming, by backend (local, s3) and \
                 operation (put, which includes the time the bytes take to come; get \
                 until the bytes can be read; head, copy, delete, rename; and on s3 the \
                 steps of an upload in parts: multipart_create, multipart_part, \
                 multipart_complete, multipart_abort); its count is the number of \
                 operations.",
            )
            .namespace(PREFIX)
            .buckets(BUCKETS.to_vec()),
            &["backend", "operation"],
        )?;
        let failures = IntCounterVec::new(
            Opts::new(
                "files_operation_failures_total",
                "How many operations on the file storage failed, by backend and \
                 operation: an error of the disk or the object store, also one while \
                 the bytes of a get were read, or a path the server made that could not \
                 be used. A file that is not there is an answer, and a put whose bytes \
                 stopped coming or went past a limit is no failure. The log says what.",
            )
            .namespace(PREFIX),
            &["backend", "operation"],
        )?;
        registry.register(Box::new(operations.clone()))?;
        registry.register(Box::new(failures.clone()))?;
        Ok(Self {
            operations,
            failures,
        })
    }
}

impl Metrics {
    /// Makes the count of the failures of every operation `backend` has, at zero if it
    /// is not there yet, so that the first failure is an increase.
    pub(crate) fn files_known(&self, backend: FilesBackend) {
        let Some(kept) = &self.kept else {
            return;
        };
        let multipart: &[FilesOperation] = match backend {
            FilesBackend::Local => &[],
            FilesBackend::S3 => &FilesOperation::MULTIPART,
        };
        for operation in FilesOperation::ON_EVERY_BACKEND.iter().chain(multipart) {
            kept.files
                .failures
                .with_label_values(&[backend.as_str(), operation.as_str()]);
        }
    }

    /// Measures an operation on the file storage that took `took`, and counts it if it
    /// failed.
    pub(crate) fn files_operated(
        &self,
        backend: FilesBackend,
        operation: FilesOperation,
        outcome: FilesOutcome,
        took: Duration,
    ) {
        let Some(kept) = &self.kept else {
            return;
        };
        let labels = [backend.as_str(), operation.as_str()];
        kept.files
            .operations
            .with_label_values(&labels)
            .observe(took.as_secs_f64());
        if outcome == FilesOutcome::Failed {
            kept.files.failures.with_label_values(&labels).inc();
        }
    }

    /// Counts a failure of an operation that was measured as answered: an error while
    /// the bytes of a get were read.
    pub(crate) fn files_failed_later(&self, backend: FilesBackend, operation: FilesOperation) {
        if let Some(kept) = &self.kept {
            kept.files
                .failures
                .with_label_values(&[backend.as_str(), operation.as_str()])
                .inc();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_labels_are_two_backends_and_ten_operations() {
        let metrics = Metrics::kept().expect("the metrics register");
        metrics.files_known(FilesBackend::Local);
        metrics.files_known(FilesBackend::S3);
        let text = String::from_utf8(
            metrics
                .render()
                .expect("the metrics are kept")
                .expect("the metrics encode"),
        )
        .expect("the text format is UTF-8");
        let failures: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("fvoci_files_operation_failures_total{"))
            .collect();
        assert_eq!(failures.len(), 6 + 10, "{text}");
        let operations = FilesOperation::ON_EVERY_BACKEND
            .iter()
            .chain(&FilesOperation::MULTIPART)
            .map(|operation| format!("operation=\"{}\"", operation.as_str()))
            .collect::<Vec<_>>();
        for line in failures {
            assert!(
                line.contains("backend=\"local\"") || line.contains("backend=\"s3\""),
                "{line}"
            );
            assert!(
                operations.iter().any(|operation| line.contains(operation)),
                "{line}"
            );
            assert!(line.ends_with(" 0"), "from zero: {line}");
        }
    }
}
