//! Wire shapes for document body, block, backlink and duplicate routes (source
//! `contracts/src/documents.ts`). The body PUT union and the block node are
//! parsed strictly from JSON in `crate::http::routes::document_body`; the input
//! structs here document them for OpenAPI.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

/// `GET …/body?format=md` (source `documentBodyMdOutput`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct BodyMdResponse {
    pub content_md: String,
    pub version: i32,
}

/// `GET …/body` answers JSON or, with `format=md`, Markdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum DocumentBodyResponse {
    Json(crate::api::dto::BodyResponse),
    Markdown(BodyMdResponse),
}

/// `PUT …/body`: exactly one of `contentJson` (Tiptap doc) or `contentMd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PutDocumentBodyInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_json: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_md: Option<String>,
}

/// `PATCH …/blocks/{blockId}` (source `documentBlockPatchInput`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PatchBlockInput {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `POST …/duplicate` (source `documentDuplicateInput`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct DuplicateDocumentInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_children: Option<bool>,
}

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
