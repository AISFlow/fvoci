//! JSON-RPC 2.0 as the MCP server speaks it: one message a request, never a batch, and
//! the errors of the protocol apart from what a tool refuses (`isError` of its result).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::error_response::SpokenInItsProtocol;

/// The body is no JSON.
pub(super) const PARSE_ERROR: i64 = -32700;
/// The JSON is no request this server takes: no JSON-RPC 2.0, a batch, no method.
pub(super) const INVALID_REQUEST: i64 = -32600;
/// No method of that name.
pub(super) const METHOD_NOT_FOUND: i64 = -32601;
/// The parameters are not what the method, or the tool, takes.
pub(super) const INVALID_PARAMS: i64 = -32602;
/// The server failed.
pub(super) const INTERNAL_ERROR: i64 = -32603;

/// An error of the protocol, answered as a JSON-RPC error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RpcError {
    /// Its code, one of those above.
    pub(super) code: i64,
    /// What went wrong.
    pub(super) message: String,
}

impl RpcError {
    pub(super) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The parameters are not what is taken.
    pub(super) fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}

/// A message an agent sent, as read.
#[derive(Debug, PartialEq)]
pub(super) enum Incoming {
    /// A request, answered with its `id`.
    Request {
        /// The identifier to answer with: a string or a number.
        id: Value,
        /// The method it calls, such as `tools/call`.
        method: String,
        /// Its parameters; an empty object if it sent none.
        params: Map<String, Value>,
    },
    /// A notification, or the answer to a request of the server's: nothing to answer.
    Accepted,
    /// A request, by its `id`, that is refused before it is done: answered with the
    /// error and the `id`, for the client to know which of its requests it was.
    Refused { id: Value, error: RpcError },
}

/// What a request whose params are no object is refused for.
const NOT_AN_OBJECT: &str = "the params are an object";

/// The error of a body that holds a batch, or other JSON than one message.
fn not_one_message() -> RpcError {
    RpcError::new(
        INVALID_REQUEST,
        "one JSON-RPC message is sent at a time: no batch, and no other JSON",
    )
}

/// Reads one message of a body.
///
/// # Errors
///
/// Returns the error to answer without an `id`: `-32700` for a body that is no JSON,
/// `-32600` for JSON that is no single JSON-RPC 2.0 message (a batch among them).
pub(super) fn read(body: &[u8]) -> Result<Incoming, RpcError> {
    // A batch is refused before it is read, whatever is in it.
    if body.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'[') {
        return Err(not_one_message());
    }
    let message: Value = serde_json::from_slice(body)
        .map_err(|error| RpcError::new(PARSE_ERROR, format!("the body is no JSON: {error}")))?;
    let Value::Object(mut message) = message else {
        return Err(not_one_message());
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(RpcError::new(INVALID_REQUEST, "jsonrpc is \"2.0\""));
    }
    let Some(method) = message.remove("method") else {
        // An answer to a request of the server's, which sends none: taken, and dropped.
        return if message.contains_key("result") || message.contains_key("error") {
            Ok(Incoming::Accepted)
        } else {
            Err(RpcError::new(INVALID_REQUEST, "a request has a method"))
        };
    };
    let Value::String(method) = method else {
        return Err(RpcError::new(INVALID_REQUEST, "the method is a string"));
    };
    let id = match message.remove("id") {
        None => None,
        Some(id @ (Value::String(_) | Value::Number(_))) => Some(id),
        Some(_) => {
            return Err(RpcError::new(
                INVALID_REQUEST,
                "the id is a string or a number",
            ));
        }
    };
    let params = match (message.remove("params"), id) {
        (None, id) => (Map::new(), id),
        (Some(Value::Object(params)), id) => (params, id),
        (Some(_), Some(id)) => {
            let error = RpcError::invalid_params(NOT_AN_OBJECT);
            return Ok(Incoming::Refused { id, error });
        }
        (Some(_), None) => return Err(RpcError::new(INVALID_REQUEST, NOT_AN_OBJECT)),
    };
    match params {
        (_, None) => Ok(Incoming::Accepted),
        (params, Some(id)) => Ok(Incoming::Request { id, method, params }),
    }
}

/// The answer to the request `id`: its result.
pub(super) fn result(id: Value, result: Value) -> Response {
    let mut message = Map::new();
    message.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    message.insert("id".to_owned(), id);
    message.insert("result".to_owned(), result);
    Json(Value::Object(message)).into_response()
}

/// The answer to the request `id`: an error of the protocol. A request that was read
/// is answered with 200, as any of its answers.
pub(super) fn error(id: &Value, error: &RpcError) -> Response {
    Json(error_message(id, error)).into_response()
}

/// The answer to a body that was no request: 400, and the error without an `id`, in
/// JSON-RPC and not as a problem.
pub(super) fn unread(error: &RpcError) -> Response {
    let mut response = (
        StatusCode::BAD_REQUEST,
        Json(error_message(&Value::Null, error)),
    )
        .into_response();
    response.extensions_mut().insert(SpokenInItsProtocol);
    response
}

fn error_message(id: &Value, error: &RpcError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": error.code, "message": error.message},
    })
}
