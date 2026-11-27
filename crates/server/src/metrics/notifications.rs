//! The metrics of the notifications: what became of each row of the list of whom a
//! change may be told to.

use prometheus::{IntCounterVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// What became of a row of the list of whom a change may be told to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationSettled {
    /// A new notification was made of it.
    Made,
    /// It joined the notification of the same card or page and kind of the last hour.
    Joined,
    /// The notification of the same card or page and kind of the last hour counted the
    /// same doer's doing in the same place already (a mention taken out and put back).
    Counted,
    /// Its doer made as many notifications for the person within the hour as one may.
    OverTheLimit,
    /// The person was not told: they did it, do not read it, or switched it off.
    NotTold,
}

impl NotificationSettled {
    const ALL: [Self; 5] = [
        Self::Made,
        Self::Joined,
        Self::Counted,
        Self::OverTheLimit,
        Self::NotTold,
    ];

    /// The value of the `outcome` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Made => "made",
            Self::Joined => "joined",
            Self::Counted => "counted",
            Self::OverTheLimit => "over_the_limit",
            Self::NotTold => "not_told",
        }
    }
}

/// The metrics of the notifications.
#[derive(Debug)]
pub(super) struct NotificationMetrics {
    settled: IntCounterVec,
}

impl NotificationMetrics {
    /// Makes the metrics and registers them in `registry`, every label value at zero.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let settled = IntCounterVec::new(
            Opts::new(
                "notifications_total",
                "What became of whom a change may be told to, by outcome: made (a new \
                 notification), joined (one of the same card or page and kind of the \
                 last hour counts it), counted (that one counted the same person's doing \
                 in the same place already), over_the_limit (whoever did it made a hundred for \
                 the person within the hour), not_told (the person did it, does not read \
                 it, or switched its kind off).",
            )
            .namespace(PREFIX),
            &["outcome"],
        )?;
        registry.register(Box::new(settled.clone()))?;
        for outcome in NotificationSettled::ALL {
            settled.with_label_values(&[outcome.as_str()]).reset();
        }
        Ok(Self { settled })
    }
}

impl Metrics {
    /// Counts a row of the list of whom to tell that was settled so.
    pub(crate) fn notification_settled(&self, outcome: NotificationSettled) {
        if let Some(kept) = &self.kept {
            kept.notifications
                .settled
                .with_label_values(&[outcome.as_str()])
                .inc();
        }
    }
}
