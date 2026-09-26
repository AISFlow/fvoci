#![cfg(feature = "db-tests")]
//! `fvoci-mcp` against a real `fvoci-server` process: a fresh database with
//! the unprivileged app role, a personal API token minted over HTTP, and the
//! MCP binary driven over stdio. Scopes, workspace binding and revocation are
//! enforced by the server; the MCP binary only relays.

#[allow(dead_code)]
mod support;

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use support::{setup_owner_session, TestDb, PEPPER, PUBLIC_ORIGIN};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use uuid::Uuid;

const TOOLS_JSON: &str = include_str!("../src/bin/fvoci-mcp/tools.json");

/// `fvoci-server` with only the settings the MCP tools need (no collab,
/// search or AI), bound to port 0.
struct ServerProcess(std::process::Child);

impl ServerProcess {
    fn kill_and_wait(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn spawn_server(harness: &TestDb) -> (ServerProcess, String, Arc<Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader};
    let storage = std::env::temp_dir().join(format!("fvoci-mcp-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .env("FVOCI_BIND", "127.0.0.1:0")
        .env("FVOCI_PUBLIC_ORIGIN", PUBLIC_ORIGIN)
        .env("FVOCI_COOKIE_SECURE", "0")
        .env("FVOCI_STORAGE_DIR", &storage)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fvoci-server");
    let logs = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let logs = logs.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                logs.lock().unwrap().push(line.clone());
                let _ = tx.send(line);
            }
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut base = None;
    while std::time::Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
            if let Some(rest) = line.strip_prefix("fvoci-server listening on ") {
                base = Some(rest.trim().trim_end_matches('/').to_string());
                break;
            }
        } else if child.try_wait().ok().flatten().is_some() {
            break;
        }
    }
    let owned = ServerProcess(child);
    let base = base.unwrap_or_else(|| panic!("server did not start: {:?}", logs.lock().unwrap()));
    (owned, base, logs)
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl Mcp {
    fn spawn(url: &str, token: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fvoci-mcp"))
            .env_clear()
            .env("FVOCI_URL", url)
            .env("FVOCI_TOKEN", token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn fvoci-mcp");
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            stdin,
            lines,
            next_id: 1,
        }
    }

    async fn send_line(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.write_all(b"\n").await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn read(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(30), self.lines.next_line())
            .await
            .expect("mcp response timeout")
            .unwrap()
            .expect("mcp closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_line(&msg.to_string()).await;
        let res = self.read().await;
        assert_eq!(res["id"], id, "{res}");
        res
    }

    async fn initialize(&mut self) -> Value {
        let res = self
            .rpc(
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "0"}}),
            )
            .await;
        self.send_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        res
    }

    /// `(isError, parsed text payload or raw text)`.
    async fn tool(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let res = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        let result = &res["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        let payload = serde_json::from_str(text).unwrap_or(Value::String(text.to_string()));
        (result["isError"] == true, payload)
    }

    async fn close(mut self) -> (i32, String) {
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("mcp exit on stdin EOF")
            .unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .await
            .unwrap();
        (status.code().unwrap_or(-1), stderr)
    }
}

async fn http(
    base: &str,
    method: reqwest::Method,
    path: &str,
    cookie: &str,
    body: Option<Value>,
) -> Value {
    let mut req = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("origin", PUBLIC_ORIGIN);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req.send().await.unwrap();
    let status = res.status();
    let json: Value = res.json().await.unwrap_or(Value::Null);
    assert!(status.is_success(), "{path}: {status} {json}");
    json
}

async fn mint_token(base: &str, cookie: &str, ws: Uuid, scopes: &[&str]) -> (String, String) {
    let created = http(
        base,
        reqwest::Method::POST,
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        cookie,
        Some(json!({"name": "mcp", "scopes": scopes})),
    )
    .await;
    (
        created["token"].as_str().unwrap().to_string(),
        created["id"].as_str().unwrap().to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_stdio_tools_against_real_server_with_scoped_pat() {
    let harness = TestDb::bootstrap().await;
    let owner = setup_owner_session(&harness).await;
    let ws = owner.workspace_id;
    let (mut server, base, logs) = spawn_server(&harness);
    let cookie = owner.session_token.clone();

    let project = http(
        &base,
        reqwest::Method::POST,
        &format!("/api/v1/workspaces/{ws}/projects"),
        &cookie,
        Some(json!({"key": "MCP", "name": "MCP", "visibility": "workspace"})),
    )
    .await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let doc = http(
        &base,
        reqwest::Method::POST,
        &format!("/api/v1/workspaces/{ws}/documents"),
        &cookie,
        Some(json!({"parentId": null, "title": "MCP doc"})),
    )
    .await;
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let (token, token_id) =
        mint_token(&base, &cookie, ws, &["documents.write", "tasks.write"]).await;

    let mut mcp = Mcp::spawn(&base, &token);
    let init = mcp.initialize().await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        init["result"]["serverInfo"],
        json!({"name": "fvoci", "version": "0.0.0"})
    );
    assert_eq!(
        init["result"]["capabilities"],
        json!({"tools": {"listChanged": true}})
    );

    // Tool inventory and schemas are the source's, verbatim.
    let listed = mcp.rpc("tools/list", json!({})).await;
    let golden: Value = serde_json::from_str(TOOLS_JSON).unwrap();
    assert_eq!(listed["result"]["tools"], golden);
    assert_eq!(golden.as_array().unwrap().len(), 21);

    // Labels: create (trimmed name), list unwraps `items`, patch.
    let (err, empty) = mcp
        .tool(
            "list_labels",
            json!({"workspaceId": ws, "projectId": project_id}),
        )
        .await;
    assert!(!err, "{empty}");
    assert_eq!(empty, json!([]));
    let (err, label) = mcp
        .tool(
            "create_label",
            json!({"workspaceId": ws, "projectId": project_id, "name": "  urgent  ", "color": "red"}),
        )
        .await;
    assert!(!err, "{label}");
    assert_eq!(label["name"], "urgent");
    let label_id = label["id"].as_str().unwrap().to_string();
    let (err, patched) = mcp
        .tool(
            "patch_label",
            json!({"workspaceId": ws, "projectId": project_id, "id": label_id, "color": "blue"}),
        )
        .await;
    assert!(!err, "{patched}");
    assert_eq!(patched, json!({"ok": true}));
    let (_, labels) = mcp
        .tool(
            "list_labels",
            json!({"workspaceId": ws, "projectId": project_id}),
        )
        .await;
    assert_eq!(labels.as_array().unwrap().len(), 1);
    assert_eq!(labels[0]["color"], "blue");

    // Tasks: create, get, patch, list with a view query, calendar, activity.
    let (err, task) = mcp
        .tool(
            "create_task",
            json!({"workspaceId": ws, "projectId": project_id, "title": "  First  ",
                   "priority": "high", "startDate": "2026-09-01", "dueDate": "2026-09-10"}),
        )
        .await;
    assert!(!err, "{task}");
    assert_eq!(task["title"], "First");
    let task_id = task["id"].as_str().unwrap().to_string();
    let (err, second) = mcp
        .tool(
            "create_task",
            json!({"workspaceId": ws, "projectId": project_id, "title": "Second"}),
        )
        .await;
    assert!(!err, "{second}");
    let second_id = second["id"].as_str().unwrap().to_string();
    let (err, got) = mcp
        .tool("get_task", json!({"workspaceId": ws, "id": task_id}))
        .await;
    assert!(!err, "{got}");
    assert_eq!(got["id"], task_id);
    let (err, patched) = mcp
        .tool(
            "patch_task",
            json!({"workspaceId": ws, "id": task_id, "title": "Renamed",
                   "labelIds": [label_id], "estimate": null}),
        )
        .await;
    assert!(!err, "{patched}");
    assert_eq!(patched["title"], "Renamed");
    let (err, listed) = mcp
        .tool(
            "list_tasks",
            json!({"workspaceId": ws, "projectId": project_id,
                   "query": {"filters": {"priority": "high"}}, "limit": 10}),
        )
        .await;
    assert!(!err, "{listed}");
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [task_id.as_str()]);
    let (err, calendar) = mcp
        .tool(
            "get_calendar",
            json!({"workspaceId": ws, "projectId": project_id, "from": "2026-09-05", "to": "2026-09-06"}),
        )
        .await;
    assert!(!err, "{calendar}");
    assert_eq!(calendar["items"].as_array().unwrap().len(), 1);
    let (err, activity) = mcp
        .tool(
            "list_task_activity",
            json!({"workspaceId": ws, "id": task_id, "filter": "changes", "limit": 20}),
        )
        .await;
    assert!(!err, "{activity}");
    assert!(!activity["items"].as_array().unwrap().is_empty());

    // Dependencies.
    let (err, dep) = mcp
        .tool(
            "add_task_dependency",
            json!({"workspaceId": ws, "id": task_id, "blockedId": second_id, "type": "FS"}),
        )
        .await;
    assert!(!err, "{dep}");
    let (err, deps) = mcp
        .tool(
            "list_task_dependencies",
            json!({"workspaceId": ws, "projectId": project_id}),
        )
        .await;
    assert!(!err, "{deps}");
    assert_eq!(deps.as_array().unwrap().len(), 1);
    let (err, removed) = mcp
        .tool(
            "remove_task_dependency",
            json!({"workspaceId": ws, "id": task_id, "blockedId": second_id}),
        )
        .await;
    assert!(!err, "{removed}");

    // Comments on a task and on a wiki document; resolve.
    let (err, comment) = mcp
        .tool(
            "add_comment",
            json!({"workspaceId": ws, "taskId": task_id, "body": "  via mcp  "}),
        )
        .await;
    assert!(!err, "{comment}");
    assert_eq!(comment["body"], "via mcp");
    let (err, doc_comment) = mcp
        .tool(
            "add_comment",
            json!({"workspaceId": ws, "documentId": doc_id, "body": "doc note"}),
        )
        .await;
    assert!(!err, "{doc_comment}");
    let (err, resolved) = mcp
        .tool(
            "resolve_comment",
            json!({"workspaceId": ws, "id": doc_comment["id"]}),
        )
        .await;
    assert!(!err, "{resolved}");
    assert!(!resolved["resolvedAt"].is_null(), "{resolved}");
    let (err, msg) = mcp
        .tool("add_comment", json!({"workspaceId": ws, "body": "orphan"}))
        .await;
    assert!(err);
    assert_eq!(msg, "exactly one of documentId or taskId is required");

    // Document body (contentJson).
    let (err, body) = mcp
        .tool(
            "get_document_body",
            json!({"workspaceId": ws, "id": doc_id}),
        )
        .await;
    assert!(!err, "{body}");
    assert_eq!(body["contentJson"]["type"], "doc");

    // Server routes not ported yet (docs/rewrite.md): PAT search, markdown
    // body, body replace and block patch. The tool relays the server's refusal
    // as a tool error with its status, never a success.
    for (tool, args) in [
        ("search", json!({"workspaceId": ws, "q": "First"})),
        (
            "get_document_body",
            json!({"workspaceId": ws, "id": doc_id, "format": "md"}),
        ),
        (
            "put_document_body",
            json!({"workspaceId": ws, "id": doc_id, "contentMd": "# x"}),
        ),
        (
            "patch_document_block",
            json!({"workspaceId": ws, "id": doc_id, "blockId": Uuid::now_v7(), "type": "paragraph"}),
        ),
    ] {
        let (err, refused) = mcp.tool(tool, args).await;
        assert!(err, "{tool}: {refused}");
        assert!(
            refused["status"].as_u64().unwrap() >= 400,
            "{tool}: {refused}"
        );
    }

    // Input validation happens before any request.
    let (err, msg) = mcp
        .tool(
            "create_task",
            json!({"workspaceId": "not-a-uuid", "projectId": project_id, "title": "x"}),
        )
        .await;
    assert!(err);
    assert!(
        msg.as_str().unwrap().starts_with(
            "MCP error -32602: Input validation error: Invalid arguments for tool create_task: workspaceId"
        ),
        "{msg}"
    );
    let (err, msg) = mcp.tool("no_such_tool", json!({})).await;
    assert!(err);
    assert_eq!(msg, "MCP error -32602: Tool no_such_tool not found");
    let unknown = mcp.rpc("resources/list", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    mcp.send_line("{not json").await;
    let parse = mcp.read().await;
    assert_eq!(parse["error"]["code"], -32700);

    // Another workspace's id with this token: the server hides it (404).
    let (err, hidden) = mcp
        .tool(
            "get_task",
            json!({"workspaceId": Uuid::now_v7(), "id": task_id}),
        )
        .await;
    assert!(err);
    assert_eq!(hidden["status"], 404, "{hidden}");

    let (code, stderr) = mcp.close().await;
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains(&token));

    // A documents.read token: task writes are refused by the server.
    let (read_token, _) = mint_token(&base, &cookie, ws, &["documents.read"]).await;
    let mut reader = Mcp::spawn(&base, &read_token);
    reader.initialize().await;
    let (err, refused) = reader
        .tool(
            "create_task",
            json!({"workspaceId": ws, "projectId": project_id, "title": "nope"}),
        )
        .await;
    assert!(err);
    assert_eq!(refused["status"], 404, "{refused}");
    let (err, body) = reader
        .tool(
            "get_document_body",
            json!({"workspaceId": ws, "id": doc_id}),
        )
        .await;
    assert!(!err, "{body}");
    let (err, refused) = reader
        .tool(
            "add_comment",
            json!({"workspaceId": ws, "documentId": doc_id, "body": "read-only"}),
        )
        .await;
    assert!(err, "{refused}");
    reader.close().await;

    // Revoked: the next call is 401 with the problem body, and no token text.
    http(
        &base,
        reqwest::Method::DELETE,
        &format!("/api/v1/workspaces/{ws}/api-tokens/{token_id}"),
        &cookie,
        None,
    )
    .await;
    let mut revoked = Mcp::spawn(&base, &token);
    revoked.initialize().await;
    let (err, denied) = revoked
        .tool("get_task", json!({"workspaceId": ws, "id": task_id}))
        .await;
    assert!(err);
    assert_eq!(denied["status"], 401, "{denied}");
    assert!(!denied.to_string().contains(&token));
    revoked.close().await;

    // The server never logged a token.
    let server_logs = logs.lock().unwrap().join("\n");
    assert!(!server_logs.contains(&token) && !server_logs.contains(&read_token));

    server.kill_and_wait();
    owner.pool.close().await;
    harness.cleanup().await.unwrap();
}

#[tokio::test]
async fn mcp_refuses_missing_env_plain_remote_http_and_arguments() {
    let secret = "pat-secret-should-never-appear";
    for (url, args, want) in [
        (None, vec![], "FVOCI_URL and FVOCI_TOKEN"),
        (Some("http://example.com"), vec![], "FVOCI_URL is invalid"),
        (Some("http://127.0.0.1:1"), vec!["--http", "8080"], "stdio"),
    ] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_fvoci-mcp"));
        cmd.env_clear().env("FVOCI_TOKEN", secret).args(&args);
        if let Some(url) = url {
            cmd.env("FVOCI_URL", url);
        }
        let out = cmd.stdin(Stdio::null()).output().await.unwrap();
        assert_eq!(out.status.code(), Some(1), "{url:?}");
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(want), "{stderr}");
        assert!(!stderr.contains(secret));
    }
}

#[tokio::test]
async fn mcp_connection_failure_is_a_tool_error_not_a_crash() {
    // A closed loopback port: the call fails as a tool error without the token.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let secret = "pat-secret-should-never-appear";
    let mut mcp = Mcp::spawn(&format!("http://127.0.0.1:{port}"), secret);
    mcp.initialize().await;
    let (err, msg) = mcp
        .tool(
            "get_task",
            json!({"workspaceId": Uuid::now_v7(), "id": Uuid::now_v7()}),
        )
        .await;
    assert!(err);
    assert_eq!(msg, "could not connect to FVOCI_URL");
    let (code, stderr) = mcp.close().await;
    assert_eq!(code, 0);
    assert!(!stderr.contains(secret));
}
