use serde::{Deserialize, Serialize};
#[cfg(feature = "api-schema")]
use utoipa::{OpenApi, ToSchema};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NativePreflightBody {
    pub archive_base64: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NativeRestoreBody {
    pub archive_base64: String,
    pub archive_hash: String,
    pub request_id: Uuid,
    pub destination_actor_id: Uuid,
    pub confirm: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NativePreflightOutput {
    pub archive_hash: String,
    pub source_workspace_id: Uuid,
    pub destination_workspace_id: Uuid,
    pub destination_actor_id: Uuid,
    pub project_id: Uuid,
    pub project_name: String,
    pub document_count: usize,
    pub task_count: usize,
    pub attachment_count: usize,
    pub revision_count: usize,
    pub complete: bool,
    pub diagnostics: Vec<String>,
    pub preserved_content_ids: bool,
    pub requires_collision_free_installation: bool,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct NativeJobOutput {
    pub id: Uuid,
    pub status: String,
    pub archive_hash: String,
    pub project_id: Option<Uuid>,
    pub diagnostic: Option<String>,
}

#[cfg(feature = "api-schema")]
#[derive(OpenApi)]
#[openapi(
    paths(export_native, preflight_native, restore_native, status_native),
    components(schemas(
        NativePreflightBody,
        NativeRestoreBody,
        NativePreflightOutput,
        NativeJobOutput
    ))
)]
pub struct NativeArchiveApi;

#[cfg(feature = "api-schema")]
#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/projects/{project_id}/native-archive",params(("workspace_id"=Uuid,Path),("project_id"=Uuid,Path),("zoteroConnector"=Option<Vec<Uuid>>,Query,description="Selected own Zotero connector; repeat the key for each (at most 8, distinct)",style=Form,explode,max_items=8)),responses((status=200,description="Persisted complete native archive",content_type="application/zip",body=String),(status=403,description="Denied"),(status=422,description="Incomplete or unsupported archive")),security(("fvoci_session"=[])))]
#[allow(dead_code)]
fn export_native() {}
#[cfg(feature = "api-schema")]
#[utoipa::path(post,path="/api/v1/workspaces/{workspace_id}/native-archive/preflight",params(("workspace_id"=Uuid,Path)),request_body=NativePreflightBody,responses((status=200,body=NativePreflightOutput),(status=409,description="Target conflict"),(status=422,description="Invalid, incomplete or unsupported archive")),security(("fvoci_session"=[])))]
#[allow(dead_code)]
fn preflight_native() {}
#[cfg(feature = "api-schema")]
#[utoipa::path(post,path="/api/v1/workspaces/{workspace_id}/native-archive/restore",params(("workspace_id"=Uuid,Path)),request_body=NativeRestoreBody,responses((status=202,body=NativeJobOutput),(status=409,description="Target or command conflict"),(status=422,description="Invalid, incomplete or unsupported archive")),security(("fvoci_session"=[])))]
#[allow(dead_code)]
fn restore_native() {}
#[cfg(feature = "api-schema")]
#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/native-archive/jobs/{job_id}",params(("workspace_id"=Uuid,Path),("job_id"=Uuid,Path)),responses((status=200,body=NativeJobOutput),(status=403,description="Denied"),(status=409,description="Generic content collision")),security(("fvoci_session"=[])))]
#[allow(dead_code)]
fn status_native() {}
