//! Typed timer transport; ordinary task metadata and estimates remain in the
//! existing task DTO. Commands carry a durable request id and expected version.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::task_timer::{TimerOperation, TimerStatus};

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerCommandBody {
    /// Intent guards, never authorization: authoritative authentication wins.
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub operation: TimerOperation,
    pub expected_version: i32,
    pub run_id: Option<Uuid>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerRunOutput {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub task_id: Uuid,
    pub status: TimerStatus,
    pub version: i32,
    pub started_at: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub running_since: Option<DateTime<Utc>>,
    /// Closed intervals only. Add the serverNow/runningSince delta for display.
    pub elapsed_milliseconds: i64,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskTimerState {
    pub server_now: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub run: Option<TimerRunOutput>,
    /// A different unfinished run, without its tenant, task id or title.
    pub busy_elsewhere: bool,
    pub legacy_open: bool,
    /// Current existing time-entry Edit/archive capability, in this snapshot.
    pub can_control: bool,
    pub actual_milliseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerCommandOutput {
    pub run_id: Uuid,
    pub version: i32,
    pub status: TimerStatus,
    pub server_now: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct OwnerTimerState {
    pub server_now: DateTime<Utc>,
    /// Own opaque run identity/version allow explicit cleanup after revocation.
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub run_id: Option<Uuid>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub version: Option<i32>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub status: Option<TimerStatus>,
    /// Present only after current task View permission is checked.
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub visible_run: Option<TimerRunOutput>,
    pub legacy_open: bool,
    pub legacy_open_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerCleanupBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub run_id: Uuid,
    pub expected_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeCorrectionBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub kind: TimeRecordKind,
    pub expected_revision: i64,
    pub expected_note: Option<String>,
    /// Original range as seen by this form; null end means a legacy open row.
    pub expected_started_at: DateTime<Utc>,
    pub expected_ended_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub note: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerDayTotal {
    pub date: NaiveDate,
    pub milliseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerSummary {
    pub server_now: DateTime<Utc>,
    pub time_zone: String,
    pub days: Vec<TimerDayTotal>,
    pub total_milliseconds: i64,
    pub unfinished: bool,
    pub unresolved_manual: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimerSummaryQuery {
    pub expected_actor_id: Option<Uuid>,
    pub expected_session_id: Option<Uuid>,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// Own task history includes unresolved manual rows explicitly, never guesses
/// their end. Projected timer rows occur once as canonical segments.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum TimeRecordKind {
    Manual,
    Segment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeRecord {
    pub id: Uuid,
    pub kind: TimeRecordKind,
    pub started_at: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub ended_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub run_id: Option<Uuid>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub note: Option<String>,
    pub revision: i64,
    pub reserved_legacy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerHistory {
    pub server_now: DateTime<Utc>,
    pub items: Vec<TimeRecord>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimerHistoryQuery {
    pub expected_actor_id: Option<Uuid>,
    pub expected_session_id: Option<Uuid>,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerManualBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub note: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimerRecordOutput {
    pub server_now: DateTime<Utc>,
    pub record: TimeRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LegacyReleaseBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub time_entry_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LegacyReleaseOutput {
    pub server_now: DateTime<Utc>,
    pub time_entry_id: Uuid,
    pub released: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimerContextQuery {
    pub expected_actor_id: Option<Uuid>,
    pub expected_session_id: Option<Uuid>,
}

#[cfg(feature = "api-schema")]
#[allow(dead_code)]
mod schema {
    use super::*;
    use crate::api::dto::ProblemResponse;
    use utoipa::OpenApi;

    #[derive(OpenApi)]
    #[openapi(
        paths(task_state, task_command, owner_state, owner_cleanup),
        components(schemas(
            TimerOperation,
            TimerStatus,
            TimerCommandBody,
            TimerCommandOutput,
            TimerRunOutput,
            TaskTimerState,
            OwnerTimerState,
            TimerCleanupBody
        ))
    )]
    pub struct TaskTimerApiDoc;

    #[utoipa::path(get, path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path),
            ("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query)),
        responses((status=200,body=TaskTimerState),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn task_state() {}
    #[utoipa::path(post, path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path)), request_body=TimerCommandBody,
        responses((status=200,body=TimerCommandOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn task_command() {}
    #[utoipa::path(get, path="/api/v1/me/task-timer", tag="tasks", security(("fvoci_session"=[])),
        params(("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query)),
        responses((status=200,body=OwnerTimerState),(status=401,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn owner_state() {}
    #[utoipa::path(post, path="/api/v1/me/task-timer/stop", tag="tasks", security(("fvoci_session"=[])), request_body=TimerCleanupBody,
        responses((status=200,body=TimerCommandOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn owner_cleanup() {}
}
#[cfg(feature = "api-schema")]
pub use schema::TaskTimerApiDoc;
