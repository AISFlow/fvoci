//! Owner-private read-only Zotero data. Remote integers cross the web as strings.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(feature = "api-schema")]
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum LibraryType {
    User,
    Group,
}
impl LibraryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
        }
    }
    pub fn path(self) -> &'static str {
        match self {
            Self::User => "users",
            Self::Group => "groups",
        }
    }
}

// No Debug/Serialize: the write-only key must never become a response or log.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ConnectBody {
    pub library_type: LibraryType,
    pub remote_library_id: String,
    pub api_key: String,
    /// Verified against every imported alternate URL. User names differ from IDs.
    pub library_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct Creator {
    pub creator_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct Tag {
    pub tag: String,
    #[serde(rename = "type", default)]
    pub tag_type: i32,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct Bibliography {
    pub item_type: String,
    pub title: String,
    pub fields: BTreeMap<String, String>,
    pub creators: Vec<Creator>,
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub relations: BTreeMap<String, Vec<String>>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ConnectorOutput {
    pub id: Uuid,
    pub library_type: LibraryType,
    pub remote_library_id: String,
    pub library_url: String,
    pub state: String,
    pub generation: String,
    pub completed_version: String,
    pub progress_version: Option<String>,
    pub committed_pages: i32,
    pub retry_at: Option<String>,
    pub reconciliation_required: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ZoteroCollectionOutput {
    pub key: String,
    pub remote_version: String,
    pub name: String,
    pub parent_key: Option<String>,
    pub availability: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LinkOutput {
    pub display_id: String,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub anchor: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ReferenceOutput {
    pub id: Uuid,
    pub document_display_id: String,
    pub connector_id: Uuid,
    pub item_key: String,
    pub remote_version: String,
    pub local_version: String,
    pub bibliography: Bibliography,
    pub return_url: String,
    pub availability: String,
    pub collection_keys: Vec<String>,
    pub links: Vec<LinkOutput>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LibraryOutput {
    pub connector: ConnectorOutput,
    pub references: Vec<ReferenceOutput>,
    pub collections: Vec<ZoteroCollectionOutput>,
}
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ConnectorListOutput {
    pub connectors: Vec<ConnectorOutput>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct LinkBody {
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub anchor: Option<String>,
    pub expected_version: String,
}
