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
