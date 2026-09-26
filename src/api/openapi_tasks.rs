// Path stubs for task time entries, clone, backlinks, purge, the flat task
// routes, parent candidates, workspace task/status lists and workflow
// statuses; merged into the main document by `spec_json`.

use utoipa::OpenApi;

use crate::api::documents_dto::PatchBlockInput;
use crate::api::dto::{
    OkResponse, PatchTaskBody, ProblemResponse, TaskListResponse, TaskMetaOutput, TaskOutput,
    WorkflowStatusOutput, WorkspaceStatusOutput,
};
use crate::api::dto::{
    RevisionCreateResponse, RevisionDetailResponse, RevisionListResponse, RevisionRestoreBody,
    RevisionRestoreResponse,
};
use crate::api::tasks_dto::{
    BacklinkListResponse, StatusCreateBody, StatusPatchBody, TaskCloneOutput,
    TaskParentCandidateOutput, TaskParentListResponse, TimeEntryCreateBody, TimeEntryListResponse,
    TimeEntryOutput, TimeEntryRollupResponse, WorkspaceStatusListResponse,
};
use crate::api::tasks_dto::{
    DocumentTaskCreateBody, DocumentTaskCreateOutput, TaskOriginItemOutput, TaskOriginListResponse,
    TaskProjectOutput, TaskProjectPickerResponse,
};

#[derive(OpenApi)]
#[openapi(
    paths(
        list_time_entries,
        create_time_entry,
        time_entries_rollup,
        clone_task,
        list_task_backlinks,
        purge_task,
        get_flat_task,
        patch_flat_task,
        delete_flat_task,
        list_task_parents,
        list_workspace_tasks,
        list_workspace_statuses,
        create_status,
        update_status,
        delete_status,
        list_task_revisions,
        create_task_revision,
        get_task_revision,
        restore_task_revision,
        patch_task_block,
        get_task_origin,
        create_document_task,
        list_document_task_projects,
        list_document_task_origins,
    ),
    components(schemas(
        StatusCreateBody,
        StatusPatchBody,
        TaskCloneOutput,
        TaskParentCandidateOutput,
        TaskParentListResponse,
        TimeEntryCreateBody,
        TimeEntryListResponse,
        TimeEntryOutput,
        TimeEntryRollupResponse,
        WorkspaceStatusListResponse,
        WorkspaceStatusOutput,
        DocumentTaskCreateBody,
        DocumentTaskCreateOutput,
        TaskOriginItemOutput,
        TaskOriginListResponse,
        TaskProjectOutput,
        TaskProjectPickerResponse,
    ))
)]
pub struct TasksApiDoc;

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Time entries, newest start first", body = TimeEntryListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_time_entries() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    request_body = TimeEntryCreateBody,
    responses(
        (status = 201, description = "Created time entry", body = TimeEntryOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "open_time_entry_exists, task_archived or project_archived", body = ProblemResponse),
    )
)]
fn create_time_entry() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries/rollup",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Total closed seconds and whether an entry is open", body = TimeEntryRollupResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn time_entries_rollup() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/clone",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "The copy in the same project", body = TaskCloneOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "task_archived or project_archived", body = ProblemResponse),
    )
)]
fn clone_task() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/backlinks",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Readable documents and tasks that reference the task", body = BacklinkListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_task_backlinks() {}

#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 200, description = "Task permanently deleted (session only)", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found, forbidden or an API token", body = ProblemResponse),
        (status = 409, description = "task_archived or project_archived", body = ProblemResponse),
    )
)]
fn purge_task() {}

#[utoipa::path(
    get,
    path = "/api/v1/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(("task_id" = String, description = "Task id")),
    responses(
        (status = 200, description = "Task detail in whichever of the actor's workspaces holds it (session only)", body = TaskOutput),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_flat_task() {}

#[utoipa::path(
    patch,
    path = "/api/v1/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(("task_id" = String, description = "Task id")),
    request_body = PatchTaskBody,
    responses(
        (status = 200, description = "Updated task (session only)", body = TaskMetaOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "Conflict", body = ProblemResponse),
    )
)]
fn patch_flat_task() {}

#[utoipa::path(
    delete,
    path = "/api/v1/tasks/{task_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(("task_id" = String, description = "Task id")),
    responses(
        (status = 200, description = "Task permanently deleted (session only)", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "task_archived or project_archived", body = ProblemResponse),
    )
)]
fn delete_flat_task() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks/parents",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("project_id" = String, description = "Project id"),
        ("childType" = String, Query, description = "Type of the task that will get the parent"),
        ("q" = Option<String>, Query, description = "Title substring or display id (max 200)"),
        ("excludeTaskId" = Option<String>, Query, description = "Task to leave out"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-20 (default 20)"),
    ),
    responses(
        (status = 200, description = "Parent candidates, most recently updated first", body = TaskParentListResponse),
        (status = 400, description = "Invalid input or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "project_archived", body = ProblemResponse),
    )
)]
fn list_task_parents() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("query" = Option<String>, Query, description = "View query JSON"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<i32>, Query, description = "Page size 1-100 (default 50)"),
    ),
    responses(
        (status = 200, description = "Tasks of every project the actor can view", body = TaskListResponse),
        (status = 400, description = "Invalid input or cursor", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_tasks() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/statuses",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(("workspace_id" = String, description = "Workspace id")),
    responses(
        (status = 200, description = "Statuses of every project the actor can view", body = WorkspaceStatusListResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn list_workspace_statuses() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("workflow_id" = String, description = "Workflow id"),
    ),
    request_body = StatusCreateBody,
    responses(
        (status = 201, description = "Status appended to the workflow", body = WorkflowStatusOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no manage permission", body = ProblemResponse),
        (status = 409, description = "workflow_status_limit or project_archived", body = ProblemResponse),
    )
)]
fn create_status() {}

#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("workflow_id" = String, description = "Workflow id"),
        ("status_id" = String, description = "Status id"),
    ),
    request_body = StatusPatchBody,
    responses(
        (status = 200, description = "Updated status", body = WorkflowStatusOutput),
        (status = 400, description = "Invalid input or anchor_not_in_target_list", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no manage permission", body = ProblemResponse),
        (status = 409, description = "project_archived", body = ProblemResponse),
    )
)]
fn update_status() {}

#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
    tag = "projects",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("workflow_id" = String, description = "Workflow id"),
        ("status_id" = String, description = "Status id"),
    ),
    responses(
        (status = 200, description = "Status deleted", body = OkResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or no manage permission", body = ProblemResponse),
        (status = 409, description = "status_has_tasks or project_archived", body = ProblemResponse),
    )
)]
fn delete_status() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("limit" = Option<i64>, Query, description = "Page size 1..=100, default 50"),
        ("cursor" = Option<String>, Query, description = "Opaque list cursor"),
    ),
    responses(
        (status = 200, description = "Task revisions, newest first", body = RevisionListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found, trashed or forbidden", body = ProblemResponse),
    )
)]
fn list_task_revisions() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
    ),
    responses(
        (status = 201, description = "Revision created (or the unchanged latest)", body = RevisionCreateResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found, no collab state, or not editable", body = ProblemResponse),
        (status = 409, description = "task_archived or project_archived", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collab unavailable", body = ProblemResponse),
    )
)]
fn create_task_revision() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("revision_id" = String, description = "Revision id"),
    ),
    responses(
        (status = 200, description = "Revision detail", body = RevisionDetailResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
    )
)]
fn get_task_revision() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore",
    tag = "tasks",
    security(("fvoci_session" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("revision_id" = String, description = "Revision id"),
    ),
    request_body = RevisionRestoreBody,
    responses(
        (status = 200, description = "Restored through the task collab room", body = RevisionRestoreResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Not found or forbidden", body = ProblemResponse),
        (status = 409, description = "restore_rejected, task_archived or project_archived", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 504, description = "Collab timeout", body = ProblemResponse),
    )
)]
fn restore_task_revision() {}

#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/blocks/{block_id}",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("block_id" = String, description = "Block id (`attrs.id`)"),
    ),
    request_body = PatchBlockInput,
    responses(
        (status = 200, description = "Task metadata after the block replace", body = TaskMetaOutput),
        (status = 400, description = "Invalid input or invalid_document_body", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Task or block not found, or not editable", body = ProblemResponse),
        (status = 409, description = "task_archived, project_archived or document_version_mismatch", body = ProblemResponse),
        (status = 413, description = "Body too large", body = ProblemResponse),
        (status = 429, description = "Rate limited", body = ProblemResponse),
        (status = 503, description = "Collab unavailable", body = ProblemResponse),
        (status = 504, description = "Collab timeout", body = ProblemResponse),
    )
)]
fn patch_task_block() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/origin",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("task_id" = String, description = "Task id"),
        ("after" = Option<String>, Query, description = "Cursor: last task id"),
        ("limit" = Option<i64>, Query, description = "Page size 1..=100, default 50"),
    ),
    responses(
        (status = 200, description = "Origin of the task; empty when the source document is not visible", body = TaskOriginListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Task not found or not visible", body = ProblemResponse),
    )
)]
fn get_task_origin() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-projects",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Source document id"),
    ),
    responses(
        (status = 200, description = "Editable target projects", body = TaskProjectPickerResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Document not visible", body = ProblemResponse),
    )
)]
fn list_document_task_projects() {}

#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/task-origins",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Source document id"),
        ("after" = Option<String>, Query, description = "Cursor: last task id"),
        ("limit" = Option<i64>, Query, description = "Page size 1..=100, default 50"),
    ),
    responses(
        (status = 200, description = "Visible linked tasks with authorized total count", body = TaskOriginListResponse),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Document not visible", body = ProblemResponse),
    )
)]
fn list_document_task_origins() {}

#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tasks",
    tag = "tasks",
    security(("fvoci_session" = []), ("bearer_api_token" = [])),
    params(
        ("workspace_id" = String, description = "Workspace id"),
        ("document_id" = String, description = "Source document id"),
    ),
    request_body = DocumentTaskCreateBody,
    responses(
        (status = 201, description = "Created task (also on an identical replay)", body = DocumentTaskCreateOutput),
        (status = 400, description = "Invalid input", body = ProblemResponse),
        (status = 401, description = "Authentication required", body = ProblemResponse),
        (status = 404, description = "Document not visible or project not editable", body = ProblemResponse),
        (status = 409, description = "document_version_mismatch (requestId reused) or project_archived", body = ProblemResponse),
    )
)]
fn create_document_task() {}
