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
    pub estimate: TaskEstimate,
}

/// Raw database timestamp preserves microseconds for compare-and-set; the
/// ordinary task DTO's millisecond timestamp is not an estimate write token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskEstimate {
    #[serde(deserialize_with = "required_nullable")]
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub value: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub unit: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskEstimateCommandBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub request_id: Uuid,
    pub expected: TaskEstimate,
    /// Null explicitly clears both the value and the unit.
    #[serde(deserialize_with = "required_nullable")]
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub minutes: Option<i32>,
    pub reason: String,
}

// Serde's default Option accepts omission. These CAS fields deliberately
// require a present key while retaining an explicit JSON null value.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Creates one ordinary task linked to an existing material/notes document.
/// The planner keeps goal/reading steps as ordinary task hierarchy and origins.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct StudyPlanTaskBody {
    pub expected_actor_id: Uuid,
    pub expected_session_id: Uuid,
    pub project_id: Uuid,
    pub request_id: Uuid,
    #[serde(default)]
    pub self_assign: bool,
    pub anchor: Option<String>,
    pub minutes: Option<i32>,
    #[cfg_attr(feature = "api-schema", schema(value_type = crate::api::dto::CreateTaskBody))]
    pub task: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct StudyPlanTaskOutput {
    pub task_id: Uuid,
    pub number: i32,
    pub project_key: String,
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
        paths(
            task_state,
            task_command,
            task_estimate,
            study_plan_task,
            study_plan_targets,
            owner_state,
            owner_cleanup,
            personal_history,
            personal_manual,
            personal_correction,
            personal_summary,
            legacy_release
        ),
        components(schemas(
            TimerOperation,
            TimerStatus,
            TimerCommandBody,
            TimerCommandOutput,
            TimerRunOutput,
            TaskTimerState,
            TaskEstimate,
            TaskEstimateCommandBody,
            StudyPlanTaskBody,
            StudyPlanTaskOutput,
            OwnerTimerState,
            TimerCleanupBody,
            TimeRecordKind,
            TimeRecord,
            TimerHistory,
            TimerSummary,
            TimerDayTotal,
            TimerManualBody,
            TimeCorrectionBody,
            TimerRecordOutput,
            LegacyReleaseBody,
            LegacyReleaseOutput
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
    #[utoipa::path(post, path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/estimate", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path)), request_body=TaskEstimateCommandBody,
        responses((status=200,body=TaskEstimate),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn task_estimate() {}
    #[utoipa::path(post, path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/study-plan/task", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("document_id"=Uuid,Path)), request_body=StudyPlanTaskBody,
        responses((status=200,body=StudyPlanTaskOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn study_plan_task() {}
    #[utoipa::path(get, path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/study-plan/task", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("document_id"=Uuid,Path),("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query)),
        responses((status=200,body=crate::api::tasks_dto::TaskProjectPickerResponse),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn study_plan_targets() {}
    #[utoipa::path(get, path="/api/v1/me/task-timer", tag="tasks", security(("fvoci_session"=[])),
        params(("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query)),
        responses((status=200,body=OwnerTimerState),(status=401,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn owner_state() {}
    #[utoipa::path(post, path="/api/v1/me/task-timer/stop", tag="tasks", security(("fvoci_session"=[])), request_body=TimerCleanupBody,
        responses((status=200,body=TimerCommandOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn owner_cleanup() {}

    #[utoipa::path(get, path="/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/history", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path),
            ("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query),
            ("from"=NaiveDate,Query),("to"=NaiveDate,Query),("cursor"=Option<String>,Query)),
        responses((status=200,body=TimerHistory),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn personal_history() {}
    #[utoipa::path(post, path="/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/history", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path)),request_body=TimerManualBody,
        responses((status=200,body=TimerRecordOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn personal_manual() {}
    #[utoipa::path(post, path="/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/records/{record_id}/correct", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path),("record_id"=Uuid,Path)),request_body=TimeCorrectionBody,
        responses((status=200,body=TimerRecordOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn personal_correction() {}
    #[utoipa::path(get, path="/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/summary", tag="tasks", security(("fvoci_session"=[])),
        params(("workspace_id"=Uuid,Path),("task_id"=Uuid,Path),
            ("expectedActorId"=Option<Uuid>,Query),("expectedSessionId"=Option<Uuid>,Query),
            ("from"=NaiveDate,Query),("to"=NaiveDate,Query)),
        responses((status=200,body=TimerSummary),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=403,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn personal_summary() {}
    #[utoipa::path(post, path="/api/v1/me/task-timer/legacy-release", tag="tasks", security(("fvoci_session"=[])),request_body=LegacyReleaseBody,
        responses((status=200,body=LegacyReleaseOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=409,body=ProblemResponse)))]
    fn legacy_release() {}
}
#[cfg(feature = "api-schema")]
pub use schema::TaskTimerApiDoc;
