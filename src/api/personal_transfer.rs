//! Explicit publication creates a current-content copy; move preserves identity.
use serde::{Deserialize, Serialize};
#[cfg(feature = "api-schema")]
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalTransferAction {
    Copy,
    Move,
}
impl PersonalTransferAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalTransferSelection {
    pub action: PersonalTransferAction,
    pub document_id: Uuid,
    pub task_id: Option<Uuid>,
    pub expected_document_version: i32,
    pub expected_task_version: Option<i32>,
    pub destination_workspace_id: Uuid,
    pub destination_project_id: Uuid,
    pub destination_status_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalTransferBody {
    pub request_id: Uuid,
    pub selection: PersonalTransferSelection,
    pub preview_digest: String,
    pub confirmed: bool,
}

/// Why an authorized transfer cannot run yet. Sent as the problem's
/// `params.code` with `personal_transfer_incomplete`; the title is only a
/// diagnostic. Each value is a known unsupported model, never a quiet subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalTransferBlocker {
    /// MOVE of a body with native state, revisions or content: complete
    /// history transfer awaits the accepted W7 retained-history contract.
    NativeHistory,
    OutgoingReference,
    IncomingReference,
    File,
    Hierarchy,
    Assignee,
    DependentGraph,
    WipReservation,
    InventoryBudget,
    BlockIdentity,
    BodyEncoding,
    NativeStateMissing,
}

/// `params.code` of `personal_transfer_conflict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalTransferConflict {
    /// The same request ID was replayed with a different command.
    CommandChanged,
    /// The source or destination changed after the reviewed preview.
    PreviewStale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalTransferItem {
    Document,
    Task,
    Activity,
    History,
    Attachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub enum PersonalTransferOutcome {
    /// Same IDs, now owned by the destination.
    Moved,
    /// A new current-content item with new IDs.
    CopiedNewId,
    /// Stays with the private original; never published.
    RetainedPrivate,
    /// Not carried over (for example, a copy starts its own activity).
    NotIncluded,
}

/// One observed part of the disclosure graph and what the command does to it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalTransferDisposition {
    pub item: PersonalTransferItem,
    pub outcome: PersonalTransferOutcome,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalTransferPreview {
    pub digest: String,
    pub document_title: String,
    pub task_title: Option<String>,
    pub workspace_name: String,
    pub project_name: String,
    pub project_visibility: String,
    pub source_retained: bool,
    pub attachment_count: u32,
    pub activity_count: u32,
    pub dispositions: Vec<PersonalTransferDisposition>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct PersonalTransferOutput {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub document_id: Uuid,
    pub document_number: i32,
    pub task_id: Option<Uuid>,
    pub task_number: Option<i32>,
    pub replayed: bool,
}

#[cfg(feature = "api-schema")]
use crate::api::dto::ProblemResponse;
#[cfg(feature = "api-schema")]
use utoipa::OpenApi;

#[cfg(feature = "api-schema")]
#[derive(OpenApi)]
#[openapi(
    paths(preview_personal_transfer, transfer_personal_item),
    components(schemas(
        PersonalTransferAction,
        PersonalTransferSelection,
        PersonalTransferBody,
        PersonalTransferPreview,
        PersonalTransferOutput,
        PersonalTransferBlocker,
        PersonalTransferConflict,
        PersonalTransferItem,
        PersonalTransferOutcome,
        PersonalTransferDisposition,
    ))
)]
pub struct PersonalTransfersApiDoc;

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/personal-transfers/preview",
    tag = "personal-transfers",
    security(("fvoci_session" = [])),
    params(("workspace_id" = Uuid, description = "Owner's personal source workspace")),
    request_body = PersonalTransferSelection,
    responses(
        (status = 200, description = "Authorized disclosure preview without writes", body = PersonalTransferPreview),
        (status = 400, description = "Invalid selection", body = ProblemResponse),
        (status = 401, description = "Session required", body = ProblemResponse),
        (status = 403, description = "Transfer authority required", body = ProblemResponse),
        (status = 404, description = "Source or destination unavailable", body = ProblemResponse),
        (status = 409, description = "personal_transfer_conflict (params.code PersonalTransferConflict) or personal_transfer_incomplete (params.code PersonalTransferBlocker)", body = ProblemResponse),
        (status = 500, description = "Transient transfer failure", body = ProblemResponse),
    )
)]
// Schema-only stubs are inspected by the OpenApi derive, never called.
#[allow(dead_code)]
fn preview_personal_transfer() {}

#[cfg(feature = "api-schema")]
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/personal-transfers",
    tag = "personal-transfers",
    security(("fvoci_session" = [])),
    params(("workspace_id" = Uuid, description = "Owner's personal source workspace")),
    request_body = PersonalTransferBody,
    responses(
        (status = 200, description = "Committed copy or move, or currently authorized command replay", body = PersonalTransferOutput),
        (status = 400, description = "Explicit confirmation and valid command required", body = ProblemResponse),
        (status = 401, description = "Session required", body = ProblemResponse),
        (status = 403, description = "Transfer authority required", body = ProblemResponse),
        (status = 404, description = "Source or destination unavailable", body = ProblemResponse),
        (status = 409, description = "personal_transfer_conflict (params.code PersonalTransferConflict) or personal_transfer_incomplete (params.code PersonalTransferBlocker)", body = ProblemResponse),
        (status = 500, description = "Transient transfer failure; no successful receipt", body = ProblemResponse),
    )
)]
#[allow(dead_code)]
fn transfer_personal_item() {}
