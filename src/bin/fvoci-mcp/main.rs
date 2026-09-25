//! `fvoci-mcp`: the product MCP server (source `apps/mcp`) over stdio.
//!
//! Reads newline-delimited JSON-RPC 2.0 from stdin and writes responses to
//! stdout; diagnostics go to stderr only. Every tool call is one request to the
//! FVOCI HTTP API with the personal API token from `FVOCI_TOKEN`; the server
//! enforces the token's scopes, workspace and the user's current permissions.
//!
//! Security: the token is read from the environment only and never printed;
//! plain http is refused except to loopback; redirects are not followed (the
//! bearer header never leaves the configured origin); input lines, API
//! responses and request time are bounded.

mod schema;
mod tools;

use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "fvoci";
const SERVER_VERSION: &str = "0.0.0";
/// Source SDK 1.30 `SUPPORTED_PROTOCOL_VERSIONS`; the first is the latest.
const PROTOCOL_VERSIONS: &[&str] = &[
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];
const TOOLS_JSON: &str = include_str!("tools.json");
/// Largest JSON-RPC message accepted on stdin.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;
/// Largest API response relayed to the client.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

const MISSING_ENV: &str = "fvoci-mcp: FVOCI_URL and FVOCI_TOKEN (documents.read/write, tasks.read/write scopes) are required";
const INVALID_URL: &str = "fvoci-mcp: FVOCI_URL is invalid";

struct Env {
    url: String,
    token: String,
}

/// Source `loadMcpEnv`: both required and trimmed; https, or http to a
/// loopback host; trailing slashes removed. Messages never echo the values.
fn load_env(url: Option<String>, token: Option<String>) -> Result<Env, &'static str> {
    let url = url.unwrap_or_default().trim().to_string();
    let token = token.unwrap_or_default().trim().to_string();
    if url.is_empty() || token.is_empty() {
        return Err(MISSING_ENV);
    }
    let parsed = url::Url::parse(&url).map_err(|_| INVALID_URL)?;
    let loopback = matches!(
        parsed.host(),
        Some(url::Host::Domain(d)) if d.eq_ignore_ascii_case("localhost")
    ) || matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip == std::net::Ipv4Addr::LOCALHOST)
        || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip == std::net::Ipv6Addr::LOCALHOST);
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => return Err(INVALID_URL),
    }
    Ok(Env {
        url: url.trim_end_matches('/').to_string(),
        token,
    })
}

struct Server {
    env: Env,
    http: reqwest::Client,
    tools: Vec<Value>,
}

enum ApiError {
    Status(u16, Value),
    Transport(String),
}

impl Server {
    fn tool(&self, name: &str) -> Option<&Value> {
        self.tools
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
    }

    async fn api(&self, call: &tools::ApiCall) -> Result<Value, ApiError> {
        let url = format!("{}{}", self.env.url, call.path);
        let method = reqwest::Method::from_bytes(call.method.as_bytes())
            .map_err(|_| ApiError::Transport("request failed".into()))?;
        let mut req = self
            .http
            .request(method, &url)
            .bearer_auth(&self.env.token)
            .header("accept", "application/json");
        if !call.query.is_empty() {
            req = req.query(&call.query);
        }
        if let Some(body) = &call.body {
            req = req.json(body);
        }
        // Transport errors can carry the URL but never headers; still keep
        // the message generic.
        let mut res = req
            .send()
            .await
            .map_err(|e| ApiError::Transport(transport_message(&e)))?;
        let status = res.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| ApiError::Transport(transport_message(&e)))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(ApiError::Transport(format!(
                    "response exceeds {MAX_RESPONSE_BYTES} bytes"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if !status.is_success() {
            return Err(ApiError::Status(status.as_u16(), parsed));
        }
        Ok(parsed)
    }

    async fn call_tool(&self, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let Some(tool) = self.tool(name) else {
            return tool_error(format!("MCP error -32602: Tool {name} not found"));
        };
        let mut args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if let Value::Object(obj) = &mut args {
            for field in tools::trimmed_fields(name) {
                if let Some(Value::String(s)) = obj.get_mut(*field) {
                    *s = s.trim().to_string();
                }
            }
        }
        let schema = &tool["inputSchema"];
        let validated = match schema::Validator::new(schema).validate_arguments(&args) {
            Ok(Value::Object(v)) => v,
            Ok(_) => Map::new(),
            Err(msg) => {
                return tool_error(format!(
                    "MCP error -32602: Input validation error: Invalid arguments for tool {name}: {msg}"
                ))
            }
        };
        let call = match tools::route(name, &validated) {
            Ok(call) => call,
            Err(msg) => return tool_error(msg),
        };
        match self.api(&call).await {
            Ok(body) => {
                let out = if call.unwrap_items {
                    match body {
                        Value::Object(mut o) if o.contains_key("items") => {
                            o.remove("items").unwrap_or(Value::Null)
                        }
                        other => other,
                    }
                } else {
                    body
                };
                json!({ "content": [{ "type": "text", "text": out.to_string() }] })
            }
            // Source `publicErrorMessage`: status and the API's problem body.
            Err(ApiError::Status(status, body)) => {
                tool_error(json!({ "status": status, "body": body }).to_string())
            }
            Err(ApiError::Transport(msg)) => tool_error(msg),
        }
    }

    /// One JSON-RPC message; `None` for notifications.
    async fn handle(&self, msg: Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(rpc_error(Value::Null, -32600, "Invalid Request"));
        };
        let id = obj.get("id").cloned();
        let method = obj.get("method").and_then(Value::as_str);
        let (Some(id), Some(method)) = (id, method) else {
            // Notifications (initialized, cancelled) and stray responses.
            return None;
        };
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(rpc_error(id, -32600, "Invalid Request"));
        }
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == requested)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({ "tools": self.tools }),
            "tools/call" => self.call_tool(&params).await,
            _ => return Some(rpc_error(id, -32601, "Method not found")),
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }
}

fn transport_message(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "request timed out".into()
    } else if err.is_connect() {
        "could not connect to FVOCI_URL".into()
    } else {
        "request failed".into()
    }
}

fn tool_error(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn run(env: Env) -> Result<(), String> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("fvoci-mcp/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "fvoci-mcp: failed to start HTTP client".to_string())?;
    let tools: Vec<Value> =
        serde_json::from_str(TOOLS_JSON).map_err(|_| "fvoci-mcp: invalid tool table")?;
    let server = Server { env, http, tools };

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = (&mut stdin)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|_| "fvoci-mcp: stdin read failed".to_string())?;
        if n == 0 {
            return Ok(());
        }
        if line.len() > MAX_LINE_BYTES {
            return Err(format!("fvoci-mcp: message exceeds {MAX_LINE_BYTES} bytes"));
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(text) {
            Ok(msg) => server.handle(msg).await,
            Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
        };
        if let Some(response) = response {
            let mut out = response.to_string();
            out.push('\n');
            stdout
                .write_all(out.as_bytes())
                .await
                .map_err(|_| "fvoci-mcp: stdout write failed".to_string())?;
            stdout
                .flush()
                .await
                .map_err(|_| "fvoci-mcp: stdout write failed".to_string())?;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        // The source's `--http` transport is not provided; never fall back
        // silently to stdio on an argument.
        eprintln!("fvoci-mcp: only the stdio transport is supported (no arguments)");
        std::process::exit(1);
    }
    let env = match load_env(
        std::env::var("FVOCI_URL").ok(),
        std::env::var("FVOCI_TOKEN").ok(),
    ) {
        Ok(env) => env,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => {
            eprintln!("fvoci-mcp: failed to start runtime");
            std::process::exit(1);
        }
    };
    let result = runtime.block_on(async {
        tokio::select! {
            r = run(env) => r,
            _ = shutdown_signal() => Ok(()),
        }
    });
    if let Err(msg) = result {
        eprintln!("{msg}");
        std::process::exit(1);
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_requires_both_and_hides_values() {
        assert_eq!(load_env(None, None).err(), Some(MISSING_ENV));
        assert_eq!(
            load_env(Some("   ".into()), Some("x".into())).err(),
            Some(MISSING_ENV)
        );
        assert_eq!(
            load_env(Some("".into()), Some("secret-pat-value".into())).err(),
            Some(MISSING_ENV)
        );
        assert!(!MISSING_ENV.contains("secret") && !INVALID_URL.contains("secret"));
    }

    #[test]
    fn env_url_rules() {
        for bad in [
            "not-a-url",
            "ftp://localhost",
            "http://example.com",
            "http://10.0.0.1:3000",
        ] {
            assert_eq!(
                load_env(Some(bad.into()), Some("x".into())).err(),
                Some(INVALID_URL),
                "{bad}"
            );
        }
        for (good, want) in [
            ("http://localhost:3000/", "http://localhost:3000"),
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000"),
            ("http://[::1]:3000", "http://[::1]:3000"),
            ("https://example.com", "https://example.com"),
        ] {
            let env = load_env(Some(good.into()), Some(" tok ".into())).unwrap();
            assert_eq!(env.url, want);
            assert_eq!(env.token, "tok");
        }
    }

    #[test]
    fn tool_table_matches_source_names() {
        let tools: Vec<Value> = serde_json::from_str(TOOLS_JSON).unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "list_tasks",
                "get_task",
                "list_task_activity",
                "create_task",
                "patch_task",
                "add_comment",
                "resolve_comment",
                "list_labels",
                "create_label",
                "patch_label",
                "list_task_dependencies",
                "add_task_dependency",
                "remove_task_dependency",
                "search",
                "get_document_body",
                "put_document_body",
                "patch_document_block",
                "get_calendar",
                "ai_summarize_document",
                "ai_generate_tasks",
                "ai_suggest_links",
            ]
        );
        // Every tool routes (no table entry without a mapping).
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            let err = tools::route(name, &Map::new()).err().unwrap_or_default();
            assert!(!err.contains("not found"), "{name}");
        }
    }

    #[test]
    fn validation_applies_defaults_trim_and_strict_nested_objects() {
        let tools: Vec<Value> = serde_json::from_str(TOOLS_JSON).unwrap();
        let schema_of =
            |n: &str| tools.iter().find(|t| t["name"] == n).unwrap()["inputSchema"].clone();
        let ws = "11111111-1111-4111-8111-111111111111";
        let list = schema_of("list_tasks");
        let v = schema::Validator::new(&list)
            .validate_arguments(
                &json!({"workspaceId": ws, "projectId": ws, "query": {}, "extra": 1}),
            )
            .unwrap();
        assert_eq!(v["query"], json!({"filters": {}, "sort": []}));
        assert!(v.get("extra").is_none());
        assert!(schema::Validator::new(&list)
            .validate_arguments(&json!({"workspaceId": ws, "projectId": ws, "query": {"bogus": 1}}))
            .is_err());
        assert!(schema::Validator::new(&list)
            .validate_arguments(&json!({"workspaceId": "nope", "projectId": ws}))
            .is_err());
        assert!(schema::Validator::new(&list)
            .validate_arguments(&json!({"workspaceId": ws, "projectId": ws, "limit": 101}))
            .is_err());
        assert!(schema::Validator::new(&list)
            .validate_arguments(&json!({"workspaceId": ws, "projectId": ws, "from": "2026-02-30"}))
            .is_err());
        assert!(schema::Validator::new(&list)
            .validate_arguments(&json!({"workspaceId": ws, "projectId": ws, "from": "٢٠٢٦-01-01"}))
            .is_err());
        let create = schema_of("create_task");
        assert!(schema::Validator::new(&create)
            .validate_arguments(&json!({"workspaceId": ws, "projectId": ws, "title": ""}))
            .is_err());
        let put = schema_of("put_document_body");
        let v = schema::Validator::new(&put)
            .validate_arguments(&json!({"workspaceId": ws, "id": ws, "contentJson": {"type": "doc", "content": [1, null, true]}}))
            .unwrap();
        assert_eq!(v["contentJson"]["type"], "doc");
        let patch = schema_of("patch_task");
        let v = schema::Validator::new(&patch)
            .validate_arguments(
                &json!({"workspaceId": ws, "id": ws, "dueDate": null, "estimate": null}),
            )
            .unwrap();
        assert_eq!(v["dueDate"], Value::Null);
    }
}
