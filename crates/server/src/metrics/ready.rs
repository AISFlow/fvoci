//! The readiness of the server as a scrape finds it: the checks the API's readiness
//! check answers with (`GET /api/v1/ready`), as gauges.

use prometheus::{IntGauge, IntGaugeVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// A check of the readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadyCheck {
    /// The database answers.
    Database,
    /// The file storage can be used.
    Files,
}

impl ReadyCheck {
    const ALL: [Self; 2] = [Self::Database, Self::Files];

    /// The value of the `check` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Files => "files",
        }
    }
}

/// The gauges of the readiness.
#[derive(Debug)]
pub(super) struct ReadyMetrics {
    ready: IntGauge,
    checks: IntGaugeVec,
}

impl ReadyMetrics {
    /// Makes the gauges and registers them in `registry`, at zero until a check.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let ready = IntGauge::with_opts(
            Opts::new(
                "ready",
                "Whether the server is ready to serve, as GET /api/v1/ready answers (kept \
                 five seconds): 1 if every check is ok, else 0.",
            )
            .namespace(PREFIX),
        )?;
        let checks = IntGaugeVec::new(
            Opts::new(
                "ready_check",
                "Whether a check of the readiness is ok (1) or failing (0), by check: \
                 database (it reads the table of the sessions in two seconds), files (the file \
                 storage can be used).",
            )
            .namespace(PREFIX),
            &["check"],
        )?;
        registry.register(Box::new(ready.clone()))?;
        registry.register(Box::new(checks.clone()))?;
        for check in ReadyCheck::ALL {
            checks.with_label_values(&[check.as_str()]).set(0);
        }
        Ok(Self { ready, checks })
    }
}

impl Metrics {
    /// Sets the readiness from how each check came out: `true` if it is ok.
    pub(crate) fn set_ready(&self, checks: [(ReadyCheck, bool); 2]) {
        let Some(kept) = &self.kept else {
            return;
        };
        for (check, ok) in checks {
            kept.ready
                .checks
                .with_label_values(&[check.as_str()])
                .set(i64::from(ok));
        }
        kept.ready
            .ready
            .set(i64::from(checks.iter().all(|(_, ok)| *ok)));
    }
}
