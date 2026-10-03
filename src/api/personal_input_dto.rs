//! Personal intent selects ordinary document/task entities, not a content format.
use serde::{Deserialize, Serialize};
#[cfg(feature = "api-schema")]
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalInputIntent {
    Quick,
    Note,
    Task,
}
impl PersonalInputIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Note => "note",
            Self::Task => "task",
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalInputSource {
    pub document_id: Uuid,
    #[serde(default)]
    pub anchor: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalInputBody {
    pub request_id: Uuid,
    pub intent: PersonalInputIntent,
    pub title: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub source: Option<PersonalInputSource>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalInputOutput {
    pub document_id: String,
    pub document_display_id: String,
    pub task_id: Option<String>,
    pub task_display_id: Option<String>,
    pub project_id: Option<String>,
    pub replayed: bool,
}
