//! The metrics of the objects of the file storage that may have no record: how many are
//! listed, being written or doomed, and how many doomed ones keep failing to go. Set
//! after each run of the housekeeping that removes them.

use prometheus::{IntGaugeVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// The states of a listed object, the values of the `state` label.
const STATES: [&str; 2] = ["writing", "doomed"];

/// The kinds of the error of a failed removal, the values of the `error` label, as the
/// logic names them; a kind it does not name here is counted as [`OTHER_ERROR`].
pub(crate) const REMOVAL_ERRORS: [&str; 5] =
    ["io", "object_store", "config", "not_found", "invalid_path"];

/// The value of the `error` label for a kind of error [`REMOVAL_ERRORS`] does not name.
const OTHER_ERROR: &str = "other";

/// The listed objects, as the housekeeping counted them after a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LooseObjects {
    /// Being written.
    pub(crate) writing: i64,
    /// Doomed: their records went, and they are to be removed.
    pub(crate) doomed: i64,
    /// The doomed ones whose removal failed again when it was tried again, by the kind
    /// of the error of the last failure.
    pub(crate) failing: Vec<(String, i64)>,
}

/// The metrics of the listed objects.
#[derive(Debug)]
pub(super) struct ObjectMetrics {
    listed: IntGaugeVec,
    failing: IntGaugeVec,
}

impl ObjectMetrics {
    /// Makes the metrics and registers them in `registry`, every label value at zero.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let listed = IntGaugeVec::new(
            Opts::new(
                "loose_objects",
                "The objects of the file storage that may have no record, by state: \
                 writing (its record is not written yet), doomed (to be removed). As of the \
                 last run of the job purge_trash, which counts them as it ends (a run that \
                 cannot count them is a failure of the job); 0 until its first run.",
            )
            .namespace(PREFIX),
            &["state"],
        )?;
        registry.register(Box::new(listed.clone()))?;
        let failing = IntGaugeVec::new(
            Opts::new(
                "loose_object_removals_failing",
                "The doomed objects whose removal failed again when it was tried again, \
                 by the kind of the error of the last try: io, object_store, config, \
                 other (not_found and invalid_path are never counted today: a removal \
                 that finds nothing has removed it, and one at a place no storage takes \
                 is taken off the list). Counted with loose_objects; 0 until then.",
            )
            .namespace(PREFIX),
            &["error"],
        )?;
        registry.register(Box::new(failing.clone()))?;
        for state in STATES {
            listed.with_label_values(&[state]).set(0);
        }
        for error in REMOVAL_ERRORS.iter().chain([&OTHER_ERROR]) {
            failing.with_label_values(&[error]).set(0);
        }
        Ok(Self { listed, failing })
    }
}

impl Metrics {
    /// Sets the counts of the listed objects.
    pub(crate) fn loose_objects_counted(&self, counted: &LooseObjects) {
        let Some(kept) = &self.kept else {
            return;
        };
        let objects = &kept.objects;
        objects
            .listed
            .with_label_values(&["writing"])
            .set(counted.writing);
        objects
            .listed
            .with_label_values(&["doomed"])
            .set(counted.doomed);
        let mut by_error = [0_i64; REMOVAL_ERRORS.len()];
        let mut other = 0_i64;
        for (error, count) in &counted.failing {
            match REMOVAL_ERRORS.iter().position(|known| known == error) {
                Some(index) => by_error[index] = by_error[index].saturating_add(*count),
                None => other = other.saturating_add(*count),
            }
        }
        for (error, count) in REMOVAL_ERRORS.iter().zip(by_error) {
            objects.failing.with_label_values(&[error]).set(count);
        }
        objects.failing.with_label_values(&[OTHER_ERROR]).set(other);
    }

    /// The count of listed objects in `state`, for tests.
    #[cfg(test)]
    fn listed_objects(&self, state: &str) -> i64 {
        self.kept.as_ref().map_or(0, |kept| {
            kept.objects.listed.with_label_values(&[state]).get()
        })
    }

    /// The count of failing removals of `error`, for tests.
    #[cfg(test)]
    fn failing_removals(&self, error: &str) -> i64 {
        self.kept.as_ref().map_or(0, |kept| {
            kept.objects.failing.with_label_values(&[error]).get()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_of_error_the_server_does_not_know_is_other() {
        let metrics = Metrics::kept().expect("the metrics register");
        metrics.loose_objects_counted(&LooseObjects {
            writing: 2,
            doomed: 5,
            failing: vec![
                ("io".to_owned(), 3),
                ("a path/that/is no kind".to_owned(), 1),
            ],
        });
        assert_eq!(
            (
                metrics.listed_objects("writing"),
                metrics.listed_objects("doomed")
            ),
            (2, 5)
        );
        assert_eq!(metrics.failing_removals("io"), 3);
        assert_eq!(metrics.failing_removals("other"), 1, "a kind not named");
        assert_eq!(
            metrics.failing_removals("object_store"),
            0,
            "a kind that did not fail is zero"
        );
        let text = String::from_utf8(
            metrics
                .render()
                .expect("kept")
                .expect("the metrics are written"),
        )
        .expect("text");
        assert!(
            !text.contains("a path"),
            "no label carries what the store said: {text}"
        );
    }
}
