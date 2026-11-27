//! The metrics of the search of bodies: what became of the searches, what became of
//! what was sent to the search server, and how much is still to be sent.

use prometheus::{IntCounterVec, IntGaugeVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// What became of a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchAsked {
    /// The search server answered, and what it answered was checked.
    Answered,
    /// The search server did not answer in time, or refused: the search was a 503.
    Unavailable,
    /// The user searched too often: the search was a 429.
    Limited,
}

impl SearchAsked {
    const ALL: [Self; 3] = [Self::Answered, Self::Unavailable, Self::Limited];

    /// The value of the `outcome` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Unavailable => "unavailable",
            Self::Limited => "limited",
        }
    }
}

/// What became of a row of the list of what the search server is to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchSent {
    /// The search server holds what is stored: the row was settled.
    Done,
    /// The search server refused a document of it: the row waits to be tried again.
    Refused,
    /// The search server did not take the request at all: the rows are as they were.
    Unavailable,
}

impl SearchSent {
    const ALL: [Self; 3] = [Self::Done, Self::Refused, Self::Unavailable];

    /// The value of the `outcome` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Refused => "refused",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The states of a row of the list, the values of the `state` label.
const STATES: [&str; 2] = ["waiting", "failing"];

/// The metrics of the search.
#[derive(Debug)]
pub(super) struct SearchMetrics {
    asked: IntCounterVec,
    sent: IntCounterVec,
    outbox: IntGaugeVec,
}

impl SearchMetrics {
    /// Makes the metrics and registers them in `registry`, every label value at zero.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let asked = IntCounterVec::new(
            Opts::new(
                "search_requests_total",
                "How many searches of bodies were asked of the search server or refused \
                 for the user's rate, by outcome: answered, unavailable (no answer in \
                 three seconds, or a refusal: a 503), limited (a 429).",
            )
            .namespace(PREFIX),
            &["outcome"],
        )?;
        registry.register(Box::new(asked.clone()))?;
        let sent = IntCounterVec::new(
            Opts::new(
                "search_changes_total",
                "What became of the changes sent to the search server, by outcome: done \
                 (the search server holds what is stored), refused (it would not take a \
                 document, which is tried again later; the log names its error), \
                 unavailable (it took no request: counted once a run, whatever was due).",
            )
            .namespace(PREFIX),
            &["outcome"],
        )?;
        registry.register(Box::new(sent.clone()))?;
        let outbox = IntGaugeVec::new(
            Opts::new(
                "search_outbox",
                "The changes the search server is still to be told, by state: waiting \
                 (all of them), failing (those it refused at least once in a row). As \
                 the worker last counted them; 0 on a server without a search server.",
            )
            .namespace(PREFIX),
            &["state"],
        )?;
        registry.register(Box::new(outbox.clone()))?;
        for outcome in SearchAsked::ALL {
            asked.with_label_values(&[outcome.as_str()]).reset();
        }
        for outcome in SearchSent::ALL {
            sent.with_label_values(&[outcome.as_str()]).reset();
        }
        for state in STATES {
            outbox.with_label_values(&[state]).set(0);
        }
        Ok(Self {
            asked,
            sent,
            outbox,
        })
    }
}

impl Metrics {
    /// Counts a search that ended so.
    pub(crate) fn search_asked(&self, outcome: SearchAsked) {
        if let Some(kept) = &self.kept {
            kept.search
                .asked
                .with_label_values(&[outcome.as_str()])
                .inc();
        }
    }

    /// Counts a change that was sent to the search server and ended so.
    pub(crate) fn search_sent(&self, outcome: SearchSent) {
        if let Some(kept) = &self.kept {
            kept.search
                .sent
                .with_label_values(&[outcome.as_str()])
                .inc();
        }
    }

    /// Sets how many changes the search server is still to be told, and how many of
    /// them it refused.
    pub(crate) fn search_outbox_counted(&self, waiting: i64, failing: i64) {
        if let Some(kept) = &self.kept {
            let outbox = &kept.search.outbox;
            outbox.with_label_values(&["waiting"]).set(waiting);
            outbox.with_label_values(&["failing"]).set(failing);
        }
    }
}
