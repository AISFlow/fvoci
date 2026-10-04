//! The typed session-only connector surface; credentials are input only.
use crate::api::dto::ProblemResponse;
use crate::api::zotero_dto::*;
use utoipa::OpenApi;
#[derive(OpenApi)]
#[openapi(
    paths(list, connect, read, disconnect, sync, link),
    components(schemas(
        LibraryType,
        ConnectBody,
        Creator,
        Tag,
        Bibliography,
        ConnectorOutput,
        ZoteroCollectionOutput,
        LinkOutput,
        ReferenceOutput,
        LibraryOutput,
        ConnectorListOutput,
        LinkBody
    ))
)]
pub struct ZoteroApiDoc;

#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/zotero",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path)),responses((status=200,body=ConnectorListOutput),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse)))]
fn list() {}
#[utoipa::path(post,path="/api/v1/workspaces/{workspace_id}/zotero",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path)),request_body=ConnectBody,responses((status=201,body=ConnectorOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse),(status=503,body=ProblemResponse)))]
fn connect() {}
#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path),("connector_id"=String,Path)),responses((status=200,body=LibraryOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse),(status=503,body=ProblemResponse)))]
fn read() {}
#[utoipa::path(delete,path="/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path),("connector_id"=String,Path)),responses((status=200,body=ConnectorOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse),(status=503,body=ProblemResponse)))]
fn disconnect() {}
#[utoipa::path(post,path="/api/v1/workspaces/{workspace_id}/zotero/libraries/{connector_id}/sync",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path),("connector_id"=String,Path)),responses((status=200,body=LibraryOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse),(status=503,body=ProblemResponse)))]
fn sync() {}
#[utoipa::path(post,path="/api/v1/workspaces/{workspace_id}/zotero/references/{reference_id}/links",tag="zotero",security(("fvoci_session"=[])),params(("workspace_id"=String,Path),("reference_id"=String,Path)),request_body=LinkBody,responses((status=200,body=LibraryOutput),(status=400,body=ProblemResponse),(status=401,body=ProblemResponse),(status=404,body=ProblemResponse),(status=409,body=ProblemResponse),(status=503,body=ProblemResponse)))]
fn link() {}
