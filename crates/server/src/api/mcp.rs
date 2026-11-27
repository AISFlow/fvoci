//! The MCP server: `POST /api/v1/mcp`, through which an agent with a personal API token
//! reads and writes what the token reaches, by tools ([`tools`]).
//!
//! It speaks the Streamable HTTP transport of the Model Context Protocol as a server
//! that keeps no session: every request is one JSON-RPC 2.0 message ([`rpc`]), answered
//! in its response as `application/json`; a notification is taken with 202. It never
//! streams and never asks the client anything, so `GET` (the stream of what the server
//! would send by itself) and `DELETE` (the end of a session) are answered 405.
//!
//! Only a personal API token is taken ([`TokenCaller`]): a session, in a cookie or as a
//! bearer, is answered 401. Each request counts against the token's limits once, as a
//! request to any route does, and each tool enters its workspace as the routes do: what
//! a token reaches, of which scope, and the diaries it never does are asked of the same
//! functions whichever way it came.

mod arguments;
mod reads;
mod rpc;
mod tools;
mod writes;

#[cfg(test)]
mod tests;

use axum::body::Bytes;
use axum::extract::{FromRequestParts, State};
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use fvoci_model::{ApiTokenScope, Problem};
use serde_json::{Map, Value, json};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use self::rpc::{Incoming, METHOD_NOT_FOUND, RpcError};
use self::tools::Failure;
use super::contract::tag;
use crate::app::{App, Caller};
use crate::auth::api_token;
use crate::client_address::ClientAddress;
use crate::error::{AppError, ErrorKind};
use crate::http_credentials::bearer_token;

/// Where the server is, under the API's prefix: the `path` of [`handle_mcp`].
pub(super) const PATH: &str = "/mcp";

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_mcp))
}

/// The versions of the protocol the server speaks, the latest last.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-03-26", "2025-06-18", "2025-11-25"];

/// The header in which a client names the version it speaks, after `initialize`.
const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

/// What the agent is told of the server when it starts.
const INSTRUCTIONS: &str = "\
fvoci keeps the boards of cards, wikis of pages and courses of units that people share in \
their workspaces. This server acts \
for the person who made the token, and reaches only the workspaces the token was given, never \
a diary. Start with list_workspaces, then get_workspace_overview for the boards, wikis and \
courses of a workspace and the properties a board's cards take. Bodies and comments are \
read and written in Markdown; a person is named @[name](fvoci-user://<id>) and a file \
[name](fvoci-file://<fileId>). To add to a body that people may be typing in, use \
appendMarkdown: it keeps what they type, where bodyMarkdown writes the body whole and is \
refused while they do. What a token may not do is refused in the tool's result (isError), \
with the reason, as the HTTP API refuses it; a refusal that came after the tool wrote \
something names it (written: its id, and whether it was made or changed), to be changed by \
its id rather than made again.";

/// The caller of a request that sends a personal API token, which is all this server
/// takes: a session, in a cookie or as a bearer, is refused.
pub(crate) struct TokenCaller(pub(crate) Caller);

impl FromRequestParts<App> for TokenCaller {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let Some(token) =
            bearer_token(&parts.headers).filter(|token| api_token::is_api_token(token))
        else {
            return Err(challenged(AppError::unauthorized(
                "the MCP server takes a personal API token, as Authorization: Bearer fvoci_pat_..., \
                 and no session",
            )));
        };
        let address = parts
            .extensions
            .get::<ClientAddress>()
            .and_then(|ClientAddress(address)| *address);
        match app.caller_by_token(token, address).await {
            Ok(caller) => Ok(Self(caller)),
            Err(error) if error.kind() == ErrorKind::Unauthorized => Err(challenged(error)),
            Err(error) => Err(error.into_response()),
        }
    }
}

/// The answer to a request that sent no token this server takes: 401, with the scheme
/// it takes.
fn challenged(error: AppError) -> Response {
    let mut response = error.into_response();
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// The Model Context Protocol
///
/// One JSON-RPC 2.0 message of the Model Context Protocol (Streamable HTTP, no session):
/// `initialize`, `ping`, `tools/list` and `tools/call` are answered with 200 and their
/// result or error as `application/json`; a notification, or an answer to the server, is
/// taken with 202 and no body. A batch is refused (`-32600`), as is a body that is no
/// JSON (`-32700`), with 400. `tools/list` lists the tools the token's scope covers: one
/// that reads only is told of no tool that writes, and is refused one it calls all the
/// same. What a tool may not do (403, 404, 409 and the like of the route it stands for)
/// is its result, marked `isError`, with the problem; its arguments not being what it
/// takes are `-32602`, and the server failing `-32603`.
///
/// Only a personal API token is taken: a request with a session, or with none, is
/// answered 401 with `WWW-Authenticate: Bearer`. A request counts against the token's
/// limits as one to any operation does, and is answered 429 past them. An
/// `MCP-Protocol-Version` header the server does not speak is answered 400.
#[utoipa::path(
    post,
    path = "/mcp",
    operation_id = "postMcp",
    tag = tag::MCP,
    request_body(
        content = Object,
        description = "One JSON-RPC 2.0 request or notification.",
        content_type = "application/json",
    ),
    responses(
        (status = 200, description = "The answer to the request: its result or its error.", body = Object),
        (status = 202, description = "A notification, or an answer to the server, taken."),
        (
            status = 400,
            description = "A body that is no single JSON-RPC message: the JSON-RPC error \
                (`-32700` or `-32600`) without an `id`, as `application/json`, as the \
                transport has it. An `MCP-Protocol-Version` header the server does not \
                speak is a problem.",
            content(
                (Object = "application/json"),
                (Problem = "application/problem+json"),
            ),
        ),
    ),
)]
async fn handle_mcp(
    State(app): State<App>,
    TokenCaller(caller): TokenCaller,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(version) = headers.get(PROTOCOL_VERSION_HEADER)
        && !version
            .to_str()
            .is_ok_and(|version| PROTOCOL_VERSIONS.contains(&version))
    {
        return AppError::bad_request(format_args!(
            "the server speaks MCP {}",
            PROTOCOL_VERSIONS.join(", ")
        ))
        .with_header("MCP-Protocol-Version")
        .into_response();
    }
    let (id, method, params) = match rpc::read(&body) {
        Ok(Incoming::Request { id, method, params }) => (id, method, params),
        Ok(Incoming::Accepted) => return StatusCode::ACCEPTED.into_response(),
        Ok(Incoming::Refused { id, error }) => return rpc::error(&id, &error),
        Err(error) => return rpc::unread(&error),
    };
    match method.as_str() {
        "initialize" => rpc::result(id, initialized(&params)),
        "ping" => rpc::result(id, json!({})),
        "tools/list" => rpc::result(id, tools::listed_for(scope_of(&caller))),
        "tools/call" => match tools::call(&app, &caller, params).await {
            Ok(result) => rpc::result(id, result),
            Err(Failure::Rpc(error)) => rpc::error(&id, &error),
            Err(Failure::Http(error)) => error.into_response(),
        },
        _ => rpc::error(
            &id,
            &RpcError::new(METHOD_NOT_FOUND, format!("there is no method {method}")),
        ),
    }
}

/// The scope of the token a caller of this server sent; that it sent one is the
/// extractor's to hold, and one that did not is taken to read only.
fn scope_of(caller: &Caller) -> ApiTokenScope {
    caller
        .delegation()
        .map_or(ApiTokenScope::Read, |delegation| delegation.scope)
}

/// The result of `initialize`: the version the client asked for if the server speaks
/// it, else the latest it does, and what the server offers.
fn initialized(params: &Map<String, Value>) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
        .unwrap_or(PROTOCOL_VERSIONS[PROTOCOL_VERSIONS.len() - 1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "fvoci", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}
