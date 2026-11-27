//! The metrics of the background work and the login limits: how often the hourly jobs
//! ran and failed, when they last ran and last did all their work, and how many attempts
//! to sign in the limits refused.

use std::time::{SystemTime, UNIX_EPOCH};

use prometheus::{GaugeVec, IntCounter, IntCounterVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// A job that runs in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    /// Purging the trash of what has been in it too long, and then removing the files
    /// no block shows any more.
    PurgeTrash,
    /// Throwing away the upload sessions that ended.
    CleanUpUploads,
}

impl Job {
    const ALL: [Self; 2] = [Self::PurgeTrash, Self::CleanUpUploads];

    /// The value of the `job` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::PurgeTrash => "purge_trash",
            Self::CleanUpUploads => "clean_up_uploads",
        }
    }
}

/// How a run of a job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobOutcome {
    /// It did all its work.
    Done,
    /// It could not do some or all of it: the store could not be read, or a file or
    /// an upload session could not be removed. The log says what.
    Failed,
}

impl JobOutcome {
    /// How a run ended that left `left` things it was to remove: done if none.
    pub(crate) fn from_left(left: usize) -> Self {
        if left == 0 { Self::Done } else { Self::Failed }
    }

    /// How a run of two parts ended: done if both were.
    #[must_use]
    pub(crate) fn and(self, other: Self) -> Self {
        if self == Self::Done && other == Self::Done {
            Self::Done
        } else {
            Self::Failed
        }
    }
}

/// A limit that refused an attempt to sign in, to register or to change a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginLimit {
    /// The account is locked after failures from anywhere.
    AccountLock,
    /// The address is made to wait at the account after failures from it.
    PairDelay,
    /// The address has an attempt in progress.
    AddressBusy,
    /// The address made too many attempts in the last minute.
    AddressRate,
}

impl LoginLimit {
    const ALL: [Self; 4] = [
        Self::AccountLock,
        Self::PairDelay,
        Self::AddressBusy,
        Self::AddressRate,
    ];

    /// The value of the `limit` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::AccountLock => "account_lock",
            Self::PairDelay => "pair_delay",
            Self::AddressBusy => "address_busy",
            Self::AddressRate => "address_rate",
        }
    }

    /// The limit's place in [`LoginLimit::ALL`].
    fn index(self) -> usize {
        match self {
            Self::AccountLock => 0,
            Self::PairDelay => 1,
            Self::AddressBusy => 2,
            Self::AddressRate => 3,
        }
    }
}

/// The metrics of the background jobs and the login limits.
#[derive(Debug)]
pub(super) struct BackgroundMetrics {
    runs: IntCounterVec,
    failures: IntCounterVec,
    last_run: GaugeVec,
    last_success: GaugeVec,
    /// The counter of each limit, in the order of [`LoginLimit::ALL`]: a refusal, which
    /// is counted while the login records are locked, is one atomic addition.
    login_refusals: [IntCounter; 4],
}

impl BackgroundMetrics {
    /// Makes the metrics and registers them in `registry`, every label value at zero.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let jobs = |name: &str, help: &str| {
            let vec = IntCounterVec::new(Opts::new(name, help).namespace(PREFIX), &["job"])?;
            registry.register(Box::new(vec.clone()))?;
            Ok::<_, prometheus::Error>(vec)
        };
        let runs = jobs(
            "job_runs_total",
            "How many times a background job ran, by job: purge_trash (the trash and the \
             files no block shows, hourly and at the start), clean_up_uploads (the upload \
             sessions that ended, at the start and hourly).",
        )?;
        let failures = jobs(
            "job_failures_total",
            "How many runs of a background job could not do all their work, by job; the \
             log says what.",
        )?;
        let last_run = GaugeVec::new(
            Opts::new(
                "job_last_run_timestamp_seconds",
                "When a background job last finished a run, by job, in seconds since \
                 1970 (UTC); 0 if it has not run yet.",
            )
            .namespace(PREFIX),
            &["job"],
        )?;
        registry.register(Box::new(last_run.clone()))?;
        let last_success = GaugeVec::new(
            Opts::new(
                "job_last_success_timestamp_seconds",
                "When a background job last finished a run that did all its work, by job, \
                 in seconds since 1970 (UTC); 0 if none has yet.",
            )
            .namespace(PREFIX),
            &["job"],
        )?;
        registry.register(Box::new(last_success.clone()))?;
        let login_refusals = IntCounterVec::new(
            Opts::new(
                "login_refusals_total",
                "How many attempts to sign in, to register or to change a password the \
                 login limits refused with 429, by limit: account_lock, pair_delay, \
                 address_busy, address_rate.",
            )
            .namespace(PREFIX),
            &["limit"],
        )?;
        registry.register(Box::new(login_refusals.clone()))?;
        for job in Job::ALL {
            runs.with_label_values(&[job.as_str()]).reset();
            failures.with_label_values(&[job.as_str()]).reset();
            last_run.with_label_values(&[job.as_str()]).set(0.0);
            last_success.with_label_values(&[job.as_str()]).set(0.0);
        }
        let login_refusals = LoginLimit::ALL.map(|limit| {
            let counter = login_refusals.with_label_values(&[limit.as_str()]);
            counter.reset();
            counter
        });
        Ok(Self {
            runs,
            failures,
            last_run,
            last_success,
            login_refusals,
        })
    }
}

impl Metrics {
    /// Counts a run of `job` that ended so, now.
    pub(crate) fn job_ran(&self, job: Job, outcome: JobOutcome) {
        let Some(kept) = &self.kept else {
            return;
        };
        let background = &kept.background;
        let job = job.as_str();
        background.runs.with_label_values(&[job]).inc();
        if outcome == JobOutcome::Failed {
            background.failures.with_label_values(&[job]).inc();
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        background.last_run.with_label_values(&[job]).set(now);
        if outcome == JobOutcome::Done {
            background.last_success.with_label_values(&[job]).set(now);
        }
    }

    /// Counts an attempt `limit` refused.
    pub(crate) fn login_refused(&self, limit: LoginLimit) {
        if let Some(kept) = &self.kept {
            kept.background.login_refusals[limit.index()].inc();
        }
    }

    /// When `job` last ran and last did all its work, for tests.
    #[cfg(test)]
    fn job_times(&self, job: Job) -> (f64, f64) {
        self.kept.as_ref().map_or((0.0, 0.0), |kept| {
            let label = [job.as_str()];
            (
                kept.background.last_run.with_label_values(&label).get(),
                kept.background.last_success.with_label_values(&label).get(),
            )
        })
    }

    /// How many attempts `limit` refused, for tests.
    #[cfg(test)]
    pub(crate) fn login_refusals(&self, limit: LoginLimit) -> u64 {
        self.kept.as_ref().map_or(0, |kept| {
            kept.background.login_refusals[limit.index()].get()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_limit_has_its_own_counter() {
        for (place, limit) in LoginLimit::ALL.into_iter().enumerate() {
            assert_eq!(limit.index(), place, "{limit:?}");
        }
    }

    #[test]
    fn a_failed_run_is_a_run_and_no_success() {
        let metrics = Metrics::kept().expect("the metrics register");
        metrics.job_ran(Job::PurgeTrash, JobOutcome::Done);
        let (ran, succeeded) = metrics.job_times(Job::PurgeTrash);
        assert!(ran > 0.0, "it ran");
        assert!(
            (succeeded - ran).abs() < f64::EPSILON,
            "and did all its work"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        metrics.job_ran(Job::PurgeTrash, JobOutcome::Failed);
        let (ran_again, still) = metrics.job_times(Job::PurgeTrash);
        assert!(ran_again > ran, "the failed run is the last run");
        assert!(
            (still - succeeded).abs() < f64::EPSILON,
            "the success stays the one before"
        );
        assert_eq!(
            metrics.job_times(Job::CleanUpUploads),
            (0.0, 0.0),
            "another job has not run"
        );
    }
}
