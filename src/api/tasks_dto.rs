//! Request/response shapes for task time entries, clone, backlinks, parent
//! candidates, the workspace task/status lists and workflow status writes
//! (source `packages/contracts/src/tasks.ts`, `documents.ts`).

use chrono::{DateTime, Utc};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::dto::{TaskMetaOutput, WorkspaceStatusOutput};

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Source `timeEntryCreateInput`: UTC ISO timestamps, `endedAt` after
/// `startedAt`, note up to 2000 characters.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeEntryCreateBody {
    pub started_at: String,
    #[serde(default, deserialize_with = "present")]
    pub ended_at: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeEntryOutput {
    pub id: String,
    pub workspace_id: String,
    pub task_id: String,
    pub user_id: String,
    pub started_at: DateTime<Utc>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub ended_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub duration_seconds: Option<i32>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeEntryListResponse {
    pub can_create: bool,
    pub items: Vec<TimeEntryOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TimeEntryRollupResponse {
    pub total_seconds: i64,
    pub open: bool,
}

/// Source `taskCloneOutput`: the copy's metadata plus its display id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskCloneOutput {
    #[serde(flatten)]
    pub meta: TaskMetaOutput,
    pub display_id: String,
}

/// Source `backlinkFrom` (same shape as the document backlinks response).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BacklinkFromResponse {
    /// `document` or `task`.
    #[serde(rename = "type")]
    pub r#type: String,
    pub id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub display_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BacklinkItemResponse {
    pub id: String,
    pub from: BacklinkFromResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BacklinkListResponse {
    pub items: Vec<BacklinkItemResponse>,
}

/// Source `taskParentQuery`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskParentQueryParams {
    pub q: Option<String>,
    pub child_type: Option<String>,
    pub exclude_task_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskParentCandidateOutput {
    pub id: String,
    pub title: String,
    pub display_id: String,
    #[serde(rename = "type")]
    pub task_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct TaskParentListResponse {
    pub items: Vec<TaskParentCandidateOutput>,
    #[cfg_attr(feature = "api-schema", schema(required = true))]
    pub next_cursor: Option<String>,
}

/// Source `workspaceTasksListQuery`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTaskListQueryParams {
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct WorkspaceStatusListResponse {
    pub items: Vec<WorkspaceStatusOutput>,
}

/// Source `statusCreateInput`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct StatusCreateBody {
    pub name: String,
    pub category: String,
    /// Integer ≥ 1, or null for no limit.
    #[serde(default, deserialize_with = "double_option")]
    #[cfg_attr(feature = "api-schema", schema(value_type = Option<i32>, nullable = true))]
    pub wip_limit: Option<Option<serde_json::Number>>,
}

/// Source `statusPatchInput`: at least one field; `beforeId` and `afterId`
/// are exclusive.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct StatusPatchBody {
    #[serde(default, deserialize_with = "present")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub category: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[cfg_attr(feature = "api-schema", schema(value_type = Option<i32>, nullable = true))]
    pub wip_limit: Option<Option<serde_json::Number>>,
    #[serde(default, deserialize_with = "present")]
    pub before_id: Option<Uuid>,
    #[serde(default, deserialize_with = "present")]
    pub after_id: Option<Uuid>,
}
