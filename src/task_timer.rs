//! Stopwatch policy. UTC instants belong to the server; local dates are a
//! presentation of intervals, never a second clock or a scheduler.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum TimerStatus {
    Running,
    Paused,
    Stopped,
}

impl TimerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum TimerOperation {
    Start,
    Pause,
    Resume,
    Stop,
}

pub fn transition(status: TimerStatus, operation: TimerOperation) -> Option<TimerStatus> {
    match (status, operation) {
        (TimerStatus::Running, TimerOperation::Pause) => Some(TimerStatus::Paused),
        (TimerStatus::Paused, TimerOperation::Resume) => Some(TimerStatus::Running),
        (TimerStatus::Running | TimerStatus::Paused, TimerOperation::Stop) => {
            Some(TimerStatus::Stopped)
        }
        _ => None,
    }
}

/// Clock regression does not manufacture negative time. The original server
/// anchor stays durable; a regressed close has zero elapsed time and its clock
/// regression is recorded in the command audit.
pub fn elapsed_milliseconds(start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    (end - start).num_milliseconds().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_is_not_stop_and_only_paused_runs_resume() {
        assert_eq!(
            transition(TimerStatus::Running, TimerOperation::Pause),
            Some(TimerStatus::Paused)
        );
        assert_eq!(
            transition(TimerStatus::Paused, TimerOperation::Resume),
            Some(TimerStatus::Running)
        );
        assert_eq!(
            transition(TimerStatus::Running, TimerOperation::Resume),
            None
        );
        assert_eq!(
            transition(TimerStatus::Stopped, TimerOperation::Resume),
            None
        );
        assert_eq!(transition(TimerStatus::Stopped, TimerOperation::Stop), None);
    }

    #[test]
    fn elapsed_uses_utc_not_wall_date_or_pause_gap() {
        let at = |value: &str| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        };
        let before = at("2026-11-01T01:59:30-04:00");
        let after = at("2026-11-01T01:00:30-05:00");
        assert_eq!(elapsed_milliseconds(before, after), 60_000);
        assert_eq!(elapsed_milliseconds(after, before), 0);
        let segments = [
            (
                at("2026-01-01T23:59:59.500Z"),
                at("2026-01-02T00:00:00.250Z"),
            ),
            (at("2026-01-02T09:00:00Z"), at("2026-01-02T09:00:00.750Z")),
        ];
        assert_eq!(
            segments
                .into_iter()
                .map(|(a, b)| elapsed_milliseconds(a, b))
                .sum::<i64>(),
            1500
        );
    }
}
