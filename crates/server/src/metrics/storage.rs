//! The metrics of the database and the files: the connection pool, the open uploads,
//! the exports and imports of archives that hold their place, and the boards and wikis
//! copied whole.

use std::fmt::Display;
use std::time::Duration;

use prometheus::{IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry};

use super::{Metrics, PREFIX, ScrapeRead};

/// How long a count of the open uploads stays fresh: a scrape before then reads none.
/// Whoever reaches the listener may scrape, and without a token anyone may; so the
/// database is asked for it at most this often, however many scrapes come. Prometheus
/// scrapes more seldom than this.
const UPLOADS_FRESH_FOR: Duration = Duration::from_secs(5);

/// The work on archives that is counted while it holds its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveWork {
    /// An archive is written and sent: its place is held until the download ends or is
    /// given up, or has stalled for `export_stall_seconds` (looked at every quarter of
    /// it), and then until its file is removed (`app/download.rs`).
    Export,
    /// An archive is read and its boards are made.
    Import,
}

impl ArchiveWork {
    /// The value of the `work` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Import => "import",
        }
    }
}

/// The work that copies a board or a wiki whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainerWork {
    /// Written into an archive that was written whole.
    Export,
    /// Made from an archive.
    Import,
    /// Copied where it is (`duplicateContainer`).
    Duplicate,
    /// The welcome board, copied for a new account (`onboard`).
    Welcome,
}

impl ContainerWork {
    const ALL: [Self; 4] = [Self::Export, Self::Import, Self::Duplicate, Self::Welcome];

    /// The value of the `work` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Import => "import",
            Self::Duplicate => "duplicate",
            Self::Welcome => "welcome",
        }
    }
}

/// What is copied: a board, a wiki, a diary or a course, the modules of a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Module {
    /// A board, with its cards and views.
    Board,
    /// A wiki, with its pages.
    Wiki,
    /// A diary: exported and imported with a user's own space, and never duplicated.
    Diary,
    /// A course: exported, imported, and duplicated whole.
    Course,
}

impl Module {
    const ALL: [Self; 4] = [Self::Board, Self::Wiki, Self::Diary, Self::Course];

    /// The value of the `kind` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Wiki => "wiki",
            Self::Diary => "diary",
            Self::Course => "course",
        }
    }
}

/// The connection pool of the database, as a scrape finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pool {
    /// The connections the pool holds, in use or idle.
    pub(crate) connections: u32,
    /// The connections the pool holds that are idle.
    pub(crate) idle: u32,
    /// The most connections the pool may hold.
    pub(crate) max_connections: u32,
}

/// The uploads in parts that are open, as a scrape finds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenUploads {
    /// How many there are.
    pub(crate) sessions: i64,
    /// The sum of their sizes, in bytes.
    pub(crate) bytes: i64,
}

/// The metrics of the database and the files.
#[derive(Debug)]
pub(super) struct StorageMetrics {
    pool_connections: IntGauge,
    pool_idle: IntGauge,
    pool_max: IntGauge,
    uploads_open: IntGauge,
    uploads_open_bytes: IntGauge,
    /// The scrapes' reads of the open uploads.
    uploads_read: ScrapeRead,
    archives: IntGaugeVec,
    copied: IntCounterVec,
}

impl StorageMetrics {
    /// Makes the metrics and registers them in `registry`.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let gauge = |name: &str, help: &str| {
            let gauge = IntGauge::with_opts(Opts::new(name, help).namespace(PREFIX))?;
            registry.register(Box::new(gauge.clone()))?;
            Ok::<_, prometheus::Error>(gauge)
        };
        let pool_connections = gauge(
            "db_pool_connections",
            "How many connections to the database the pool holds, in use or idle.",
        )?;
        let pool_idle = gauge(
            "db_pool_idle_connections",
            "How many of the pool's connections to the database are idle.",
        )?;
        let pool_max = gauge(
            "db_pool_max_connections",
            "How many connections to the database the pool may hold.",
        )?;
        let uploads_open = gauge(
            "uploads_open",
            "How many uploads in parts are open, as a scrape counts them at most once \
             in five seconds: neither completed, abandoned nor ended.",
        )?;
        let uploads_open_bytes = gauge(
            "uploads_open_bytes",
            "The sizes of the open uploads together, in bytes, counted with them: what \
             they hold back of max_pending_upload_bytes.",
        )?;
        let uploads_read = ScrapeRead::register(
            registry,
            "uploads_read_failures_total",
            "How many counts of the open uploads failed (the database failed or took \
             over two seconds); the scrapes until the next count give the values counted \
             before.",
            UPLOADS_FRESH_FOR,
        )?;
        let archives = IntGaugeVec::new(
            Opts::new(
                "archives_in_progress",
                "How many exports and imports of archives hold their place, as \
                 max_concurrent_exports and max_concurrent_imports count them (an export \
                 until its download ends), by work: export, import.",
            )
            .namespace(PREFIX),
            &["work"],
        )?;
        registry.register(Box::new(archives.clone()))?;
        // Both values are there from the start, at zero.
        for work in [ArchiveWork::Export, ArchiveWork::Import] {
            archives.with_label_values(&[work.as_str()]).set(0);
        }
        let copied = IntCounterVec::new(
            Opts::new(
                "containers_copied_total",
                "How many containers were copied whole, by work: export (written into an \
                 archive, once the archive is ready to be sent), import (made from an \
                 archive with its members), duplicate (copied by duplicateContainer), \
                 welcome (the welcome board, copied for a new account); and by kind: board, \
                 wiki, diary, course.",
            )
            .namespace(PREFIX),
            &["work", "kind"],
        )?;
        registry.register(Box::new(copied.clone()))?;
        for work in ContainerWork::ALL {
            for kind in Module::ALL {
                copied
                    .with_label_values(&[work.as_str(), kind.as_str()])
                    .reset();
            }
        }
        Ok(Self {
            pool_connections,
            pool_idle,
            pool_max,
            uploads_open,
            uploads_open_bytes,
            uploads_read,
            archives,
            copied,
        })
    }
}

impl Metrics {
    /// Sets how many exports and how many imports of archives hold their place.
    pub(crate) fn set_archives_running(&self, exports: usize, imports: usize) {
        let Some(kept) = &self.kept else {
            return;
        };
        for (work, running) in [
            (ArchiveWork::Export, exports),
            (ArchiveWork::Import, imports),
        ] {
            kept.storage
                .archives
                .with_label_values(&[work.as_str()])
                .set(i64::try_from(running).unwrap_or(i64::MAX));
        }
    }

    /// Counts a board or a wiki copied whole by `work`.
    pub(crate) fn container_copied(&self, work: ContainerWork, kind: Module) {
        if let Some(kept) = &self.kept {
            kept.storage
                .copied
                .with_label_values(&[work.as_str(), kind.as_str()])
                .inc();
        }
    }

    /// Sets the connection pool of the database.
    pub(crate) fn set_pool(&self, pool: Pool) {
        let Some(kept) = &self.kept else {
            return;
        };
        let storage = &kept.storage;
        storage.pool_connections.set(i64::from(pool.connections));
        storage.pool_idle.set(i64::from(pool.idle));
        storage.pool_max.set(i64::from(pool.max_connections));
    }

    /// Sets the open uploads to what `read` answers, for a scrape. `read` is what asks
    /// the database; it is not polled if no metrics are kept, a read is under way, or
    /// the last one started less than five seconds ago.
    /// The uploads stay as they were then, and if `read` fails or does not answer in
    /// two seconds, which is counted; a read that answers late is kept then.
    pub(crate) async fn read_open_uploads<E: Display + Send + 'static>(
        &self,
        read: impl Future<Output = Result<OpenUploads, E>> + Send + 'static,
    ) {
        let Some(kept) = &self.kept else {
            return;
        };
        let storage = &kept.storage;
        let (sessions, bytes) = (
            storage.uploads_open.clone(),
            storage.uploads_open_bytes.clone(),
        );
        let keep = move |open: OpenUploads| {
            sessions.set(open.sessions);
            bytes.set(open.bytes);
        };
        storage.uploads_read.read("open_uploads", read, keep).await;
    }
}
