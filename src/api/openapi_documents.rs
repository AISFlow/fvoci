// Path stubs for document body, block, children, backlinks, duplicate, project
// ancestors and flat document routes; merged into the main document by `spec_json`.

use utoipa::OpenApi;

use crate::api::documents_dto::{
    BacklinkFromResponse, BacklinkItemResponse, BacklinkListResponse, BodyMdResponse,
    DocumentBodyResponse, DuplicateDocumentInput, PatchBlockInput, PutDocumentBodyInput,
};
use crate::api::dto::{
    AncestorsResponse, DocumentMetaResponse, OkResponse, PatchDocumentBody, ProblemResponse,
    TreeResponse,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        put_body,
        put_project_body,
        patch_block,
        patch_project_block,
        children,
        project_children,
        backlinks,
        project_backlinks,
        duplicate,
        project_duplicate,
        project_ancestors,
        get_by_id,
        update_by_id,
        remove_by_id,
        duplicate_by_id,
    ),
    components(schemas(
        BacklinkFromResponse,
        BacklinkItemResponse,
        BacklinkListResponse,
        BodyMdResponse,
        DocumentBodyResponse,
        DuplicateDocumentInput,
        PatchBlockInput,
        PutDocumentBodyInput,
    ))
)]
pub struct DocumentsApiDoc;

#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/body",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = PutDocumentBodyInput,
    responses(
        (status = 200, description = "Document metadata after the write", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input or document body", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Concurrent edit kept moving the document", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
        (status = 504, description = "Collaboration timeout; retry", body = ProblemResponse),
    )
)]
fn put_body() {}

#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/body",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = PutDocumentBodyInput,
    responses(
        (status = 200, description = "Document metadata after the write", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input or document body", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Concurrent edit kept moving the document", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
        (status = 504, description = "Collaboration timeout; retry", body = ProblemResponse),
    )
)]
fn put_project_body() {}

#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/blocks/{block_id}",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
        ("block_id" = String, description = "Block id (attrs.id of a unique-id node)"),
    ),
    request_body = PatchBlockInput,
    responses(
        (status = 200, description = "Document metadata after the write", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input or document body", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Concurrent edit kept moving the document", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
        (status = 504, description = "Collaboration timeout; retry", body = ProblemResponse),
    )
)]
fn patch_block() {}

#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/blocks/{block_id}",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
        ("block_id" = String, description = "Block id (attrs.id of a unique-id node)"),
    ),
    request_body = PatchBlockInput,
    responses(
        (status = 200, description = "Document metadata after the write", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input or document body", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Concurrent edit kept moving the document", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
        (status = 504, description = "Collaboration timeout; retry", body = ProblemResponse),
    )
)]
fn patch_project_block() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/children",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Visible child documents", body = TreeResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn children() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/children",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Visible child documents", body = TreeResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn project_children() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/backlinks",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Readable documents and tasks referencing this document", body = BacklinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn backlinks() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/backlinks",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Readable documents and tasks referencing this document", body = BacklinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn project_backlinks() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/duplicate",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = DuplicateDocumentInput,
    responses(
        (status = 201, description = "Copy (and copied descendants)", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
    )
)]
fn duplicate() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/duplicate",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.write"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    request_body = DuplicateDocumentInput,
    responses(
        (status = 201, description = "Copy (and copied descendants)", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
    )
)]
fn project_duplicate() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/ancestors",
    tag = "documents",
    security(("fvoci_session" = []), ("bearer_api_token" = ["documents.read"])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Ancestor chain", body = AncestorsResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn project_ancestors() {}

#[utoipa::path(
    get,
    path = "/api/v1/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("document_id" = String, description = "Document id"),
    ),
    responses(
        (status = 200, description = "Document metadata", body = DocumentMetaResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_by_id() {}

#[utoipa::path(
    patch,
    path = "/api/v1/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("document_id" = String, description = "Document id"),
    ),
    request_body = PatchDocumentBody,
    responses(
        (status = 200, description = "Updated metadata", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn update_by_id() {}

#[utoipa::path(
    delete,
    path = "/api/v1/documents/{document_id}",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("document_id" = String, description = "Document id"),
        ("children" = Option<String>, Query, description = "trash (default) or reparent"),
    ),
    responses(
        (status = 200, description = "Trashed", body = OkResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn remove_by_id() {}

#[utoipa::path(
    post,
    path = "/api/v1/documents/{document_id}/duplicate",
    tag = "documents",
    security(("fvoci_session" = [])),
    params(
        ("document_id" = String, description = "Document id"),
    ),
    request_body = DuplicateDocumentInput,
    responses(
        (status = 201, description = "Copy (and copied descendants)", body = DocumentMetaResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 413, description = "Document body too large", body = ProblemResponse),
        (status = 503, description = "Collaboration unavailable", body = ProblemResponse),
    )
)]
fn duplicate_by_id() {}
