//! Wire shapes for document tags, collections and saved views (source
//! `contracts/src/{collections,documents,tasks}.ts`). Request bodies with
//! unions or nested view queries are parsed strictly from JSON in
//! `crate::collections`; the body structs here document them for OpenAPI.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagOutput {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    /// Label color key (`gray`, `red`, … `pink`).
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagPoolItemOutput {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
    pub assignment_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagPoolListResponse {
    pub can_create: bool,
    pub can_manage: bool,
    pub items: Vec<DocumentTagPoolItemOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagListResponse {
    pub items: Vec<DocumentTagOutput>,
}

/// Source `documentTagCreateInput`: `color` defaults to `gray`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagCreateBody {
    pub name: String,
    pub color: Option<String>,
}

/// Source `documentTagPatchInput`: at least one of name/color.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagPatchBody {
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DocumentTagAssignBody {
    pub tag_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionOutput {
    pub id: String,
    pub workspace_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
    /// `document` | `task`
    pub kind: String,
    pub name: String,
    pub version: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionListResponse {
    pub items: Vec<CollectionOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectCollectionOutput {
    pub id: String,
    pub workspace_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub version: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub deleted_at: Option<String>,
    pub can_edit: bool,
    pub can_manage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionOptionOutput {
    pub id: String,
    pub key: String,
    pub label: String,
    pub sort_key: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionFieldOutput {
    pub id: String,
    pub collection_id: String,
    pub key: String,
    pub name: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub description: Option<String>,
    /// `text` | `paragraph` | `number` | `date` | `datetime` | `checkbox` |
    /// `select` | `multi_select` | `checkboxes` | `user` | `user_multi` | `labels`
    pub r#type: String,
    pub version: i32,
    pub sort_key: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub deleted_at: Option<String>,
    pub options: Vec<CollectionOptionOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionFieldListResponse {
    pub items: Vec<CollectionFieldOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionItemOutput {
    pub id: String,
    pub collection_id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub document_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub task_id: Option<String>,
    pub version: i32,
}

/// `item: null` when the readable document/task is in no collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionItemLookupResponse {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub item: Option<CollectionItemOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionValueResponse {
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryItemOutput {
    pub id: String,
    pub can_edit: bool,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub document_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub task_id: Option<String>,
    pub display_id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub task_type: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub status_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub start_date: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub due_date: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub due_at: Option<String>,
    pub version: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub group: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub date: Option<String>,
    /// Field id → collection value (`{text}`, `{number}`, `{date}`, `{datetime}`,
    /// `{checkbox}`, `{options}`, `{users}`).
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub values: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryPreviewOutput {
    pub id: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub document_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub task_id: Option<String>,
    pub display_id: String,
    pub title: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub status_id: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub date: Option<String>,
    pub can_edit: bool,
    pub version: i32,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub start_date: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub due_date: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub due_at: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub values: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryGroupOutput {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub id: Option<String>,
    pub name: String,
    pub item_ids: Vec<String>,
    pub count: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryDayOutput {
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub date: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryResponse {
    pub can_edit: bool,
    pub days: Vec<CollectionQueryDayOutput>,
    pub items: Vec<CollectionQueryItemOutput>,
    pub groups: Vec<CollectionQueryGroupOutput>,
    pub count: i64,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub next_cursor: Option<String>,
    pub previews: Vec<CollectionQueryPreviewOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionViewOutput {
    pub id: String,
    pub collection_id: String,
    pub owner_id: String,
    pub version: i32,
    pub name: String,
    /// `table` | `board` | `calendar`
    pub r#type: String,
    /// `private` | `shared`
    pub visibility: String,
    /// `{ query: ViewQuery, groupBy, dateBy }`
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionViewListResponse {
    pub can_save: bool,
    pub can_manage: bool,
    pub items: Vec<CollectionViewOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectViewOutput {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// `list` | `board` | `calendar` | `gantt` | `table`
    pub r#type: String,
    /// View query `{ filters, sort }`.
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectViewListResponse {
    pub items: Vec<ProjectViewOutput>,
}

// Request bodies below document the JSON shapes that `crate::collections`
// parses strictly (the handlers read `serde_json::Value`).

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionCreateBody {
    pub name: String,
    /// `document` (task collections are created with their project)
    pub kind: String,
    #[cfg_attr(feature = "api-schema", schema(required = true, nullable = true))]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionFieldCreateBody {
    pub name: String,
    pub key: Option<String>,
    pub r#type: String,
    pub description: Option<String>,
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionOptionPatch {
    pub id: Option<String>,
    pub label: String,
    pub deleted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionFieldPatchBody {
    pub expected_version: i32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub deleted: Option<bool>,
    pub options: Option<Vec<CollectionOptionPatch>>,
}

/// Exactly one of `documentId` / `taskId`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionAttachBody {
    pub document_id: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionValueBody {
    pub field_id: String,
    pub expected_version: i32,
    pub expected_field_version: i32,
    /// `null` or one of `{text}`, `{number}`, `{date}`, `{datetime}`,
    /// `{checkbox}`, `{options: uuid[]}`, `{users: uuid[]}`.
    #[cfg_attr(feature = "api-schema", schema(value_type = Object, nullable = true))]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryWindow {
    pub from: String,
    pub to: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionQueryBody {
    /// `{ query: ViewQuery, groupBy, dateBy }`
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub config: Value,
    pub group: Option<String>,
    pub day: Option<String>,
    pub window: Option<CollectionQueryWindow>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
}

/// `expectedVersion` is required on update and rejected on create.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct CollectionViewBody {
    pub name: String,
    pub r#type: String,
    pub visibility: String,
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub config: Value,
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectViewCreateBody {
    pub name: String,
    pub r#type: String,
    #[cfg_attr(feature = "api-schema", schema(value_type = Object))]
    pub config: Value,
}

/// A `config` change requires the `expectedConfig` last read (compare-and-swap).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ProjectViewPatchBody {
    pub name: Option<String>,
    #[cfg_attr(feature = "api-schema", schema(value_type = Option<Object>))]
    pub config: Option<Value>,
    #[cfg_attr(feature = "api-schema", schema(value_type = Option<Object>))]
    pub expected_config: Option<Value>,
}
