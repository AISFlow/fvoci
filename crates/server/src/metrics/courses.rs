//! Low-cardinality Prometheus counters for learning courses: course creation
//! (blank, template, duplicate, import), unit completion toggles, and shared/personal
//! schedule redistributions.
//!
//! Every label comes from a fixed enum in this module; no `org_id`, `course_id`, or
//! `user_id` is ever recorded.

use prometheus::{IntCounterVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// How a non-template course came into existence (`source` label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CourseSource {
    /// Created empty via `createCourse`.
    Blank,
    /// Created from a built-in or workspace course template via `duplicateContainer`.
    Template,
    /// Duplicated from an existing course via `duplicateContainer`.
    Duplicate,
    /// Imported from an archive via `importArchive`.
    Import,
}

impl CourseSource {
    const ALL: [Self; 4] = [Self::Blank, Self::Template, Self::Duplicate, Self::Import];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Template => "template",
            Self::Duplicate => "duplicate",
            Self::Import => "import",
        }
    }
}

/// Direction of a personal unit completion update (`action` label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitCompletionAction {
    /// Learner marked a unit completed (`completed: true`).
    Completed,
    /// Learner cleared a unit's completion (`completed: false`).
    Uncompleted,
}

impl UnitCompletionAction {
    const ALL: [Self; 2] = [Self::Completed, Self::Uncompleted];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Uncompleted => "uncompleted",
        }
    }
}

/// Which schedule redistribution path ran (`scope` label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleRedistributionScope {
    /// Automatic proportional assignment on `putCourseSchedule` (`autoAssign: true`).
    SharedAuto,
    /// Explicit shared redistribution via `redistributeCourseSchedule`.
    SharedManual,
    /// Explicit learner-only redistribution via `redistributeMyCourseSchedule`.
    Personal,
}

impl ScheduleRedistributionScope {
    const ALL: [Self; 3] = [Self::SharedAuto, Self::SharedManual, Self::Personal];

    const fn as_str(self) -> &'static str {
        match self {
            Self::SharedAuto => "shared_auto",
            Self::SharedManual => "shared_manual",
            Self::Personal => "personal",
        }
    }
}

/// The Prometheus counters for learning courses.
#[derive(Debug)]
pub(super) struct CourseMetrics {
    created: IntCounterVec,
    unit_completions: IntCounterVec,
    schedule_redistributions: IntCounterVec,
}

impl CourseMetrics {
    /// Registers every course counter in `registry` and initializes all label combinations to 0.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let created = IntCounterVec::new(
            Opts::new(
                "courses_created_total",
                "How many courses were created, by source: blank (createCourse), \
                 template (copied from a course template), duplicate (copied from an \
                 existing course), or import (restored from an archive).",
            )
            .namespace(PREFIX),
            &["source"],
        )?;
        registry.register(Box::new(created.clone()))?;
        for source in CourseSource::ALL {
            created.with_label_values(&[source.as_str()]).reset();
        }

        let unit_completions = IntCounterVec::new(
            Opts::new(
                "course_unit_completions_total",
                "How many personal unit completion updates succeeded, by action: \
                 completed or uncompleted.",
            )
            .namespace(PREFIX),
            &["action"],
        )?;
        registry.register(Box::new(unit_completions.clone()))?;
        for action in UnitCompletionAction::ALL {
            unit_completions
                .with_label_values(&[action.as_str()])
                .reset();
        }

        let schedule_redistributions = IntCounterVec::new(
            Opts::new(
                "course_schedule_redistributions_total",
                "How many course schedule redistribution runs succeeded, by scope: \
                 shared_auto (putCourseSchedule autoAssign), shared_manual \
                 (redistributeCourseSchedule), or personal (redistributeMyCourseSchedule).",
            )
            .namespace(PREFIX),
            &["scope"],
        )?;
        registry.register(Box::new(schedule_redistributions.clone()))?;
        for scope in ScheduleRedistributionScope::ALL {
            schedule_redistributions
                .with_label_values(&[scope.as_str()])
                .reset();
        }

        Ok(Self {
            created,
            unit_completions,
            schedule_redistributions,
        })
    }
}

impl Metrics {
    /// Records one course created from `source`.
    pub(crate) fn course_created(&self, source: CourseSource) {
        if let Some(kept) = &self.kept {
            kept.courses
                .created
                .with_label_values(&[source.as_str()])
                .inc();
        }
    }

    /// Records one personal unit completion toggle (`completed` or `uncompleted`).
    pub(crate) fn course_unit_completion(&self, action: UnitCompletionAction) {
        if let Some(kept) = &self.kept {
            kept.courses
                .unit_completions
                .with_label_values(&[action.as_str()])
                .inc();
        }
    }

    /// Records one successful course schedule redistribution run.
    pub(crate) fn course_schedule_redistributed(&self, scope: ScheduleRedistributionScope) {
        if let Some(kept) = &self.kept {
            kept.courses
                .schedule_redistributions
                .with_label_values(&[scope.as_str()])
                .inc();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(metrics: &Metrics) -> String {
        String::from_utf8(
            metrics
                .render()
                .expect("kept metrics render")
                .expect("valid prometheus text"),
        )
        .expect("utf-8")
    }

    #[test]
    fn course_metrics_preinitialize_zero_and_increment_without_high_cardinality_labels() {
        let metrics = Metrics::kept().expect("metrics register");
        let initial = rendered(&metrics);
        for line in [
            r#"fvoci_courses_created_total{source="blank"} 0"#,
            r#"fvoci_courses_created_total{source="template"} 0"#,
            r#"fvoci_courses_created_total{source="duplicate"} 0"#,
            r#"fvoci_courses_created_total{source="import"} 0"#,
            r#"fvoci_course_unit_completions_total{action="completed"} 0"#,
            r#"fvoci_course_unit_completions_total{action="uncompleted"} 0"#,
            r#"fvoci_course_schedule_redistributions_total{scope="shared_auto"} 0"#,
            r#"fvoci_course_schedule_redistributions_total{scope="shared_manual"} 0"#,
            r#"fvoci_course_schedule_redistributions_total{scope="personal"} 0"#,
        ] {
            assert!(initial.contains(line), "missing initial series {line}");
        }

        metrics.course_created(CourseSource::Blank);
        metrics.course_created(CourseSource::Template);
        metrics.course_created(CourseSource::Template);
        metrics.course_created(CourseSource::Duplicate);
        metrics.course_created(CourseSource::Import);
        metrics.course_unit_completion(UnitCompletionAction::Completed);
        metrics.course_unit_completion(UnitCompletionAction::Completed);
        metrics.course_unit_completion(UnitCompletionAction::Uncompleted);
        metrics.course_schedule_redistributed(ScheduleRedistributionScope::SharedAuto);
        metrics.course_schedule_redistributed(ScheduleRedistributionScope::SharedManual);
        metrics.course_schedule_redistributed(ScheduleRedistributionScope::Personal);

        let after = rendered(&metrics);
        for line in [
            r#"fvoci_courses_created_total{source="blank"} 1"#,
            r#"fvoci_courses_created_total{source="template"} 2"#,
            r#"fvoci_courses_created_total{source="duplicate"} 1"#,
            r#"fvoci_courses_created_total{source="import"} 1"#,
            r#"fvoci_course_unit_completions_total{action="completed"} 2"#,
            r#"fvoci_course_unit_completions_total{action="uncompleted"} 1"#,
            r#"fvoci_course_schedule_redistributions_total{scope="shared_auto"} 1"#,
            r#"fvoci_course_schedule_redistributions_total{scope="shared_manual"} 1"#,
            r#"fvoci_course_schedule_redistributions_total{scope="personal"} 1"#,
        ] {
            assert!(after.contains(line), "missing updated series {line}");
        }
        for forbidden in ["org_id=", "course_id=", "user_id=", "workspace_id="] {
            assert!(
                !after.contains(forbidden),
                "high-cardinality label {forbidden} must never appear"
            );
        }
    }

    #[test]
    fn disabled_metrics_ignore_course_events() {
        let none = Metrics::none();
        none.course_created(CourseSource::Blank);
        none.course_unit_completion(UnitCompletionAction::Completed);
        none.course_schedule_redistributed(ScheduleRedistributionScope::Personal);
        assert!(none.render().is_none());
    }
}
