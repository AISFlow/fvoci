#![cfg(feature = "db-tests")]
//! Document body, block, children, backlinks, duplicate, project ancestors and
//! flat document routes over real HTTP, PostgreSQL (app role), the native collab
//! helper and the editor convert helper.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use futures_util::SinkExt;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::scopes::ApiTokenScope;
use fvoci_server::auth::token::new_token;
use fvoci_server::collab::room::arm_append_revoke_barrier;
use fvoci_server::collab::wire::{CollabKind, CollabRoomName};
use fvoci_server::db::api_tokens::{create_api_token, CreateApiTokenInput};
use fvoci_server::db::documents::CreateDocumentInput;
use fvoci_server::db::pool;
use fvoci_server::db::workspace::{self, WorkspaceRole};
use fvoci_server::http::state::AppState;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, collab_app_state, complete_sync_handshake, connect_member, engine_fixture,
    setup_owner_session, setup_wiki_doc, stateless_frame, sync_update_frame, test_collab_config,
    wait_for_stateless_exact, wait_for_sync_applied, wait_for_sync_update, SessionFixture, TestDb,
    TestRun, WikiDocFixture, PEPPER, PUBLIC_ORIGIN,
};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

async fn run_test<F>(name: &str, case: F)
where
    F: std::future::Future<Output = ()>,
{
    tokio::time::timeout(TEST_TIMEOUT, case)
        .await
        .unwrap_or_else(|_| panic!("{name} hung (>{TEST_TIMEOUT:?})"));
}

async fn app_state(
    harness: &TestDb,
) -> (AppState, std::sync::Arc<fvoci_server::collab::CollabHub>) {
    // No Node helper: body writes, Markdown and duplicate seeds are Rust children.
    let (state, hub) = collab_app_state(&harness.app_url, test_collab_config(8, 60_000)).await;
    assert!(state.document_convert.is_none());
    (state, hub)
}

fn routing_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

enum Cred<'a> {
    Session(&'a str),
    Bearer(&'a str),
}

async fn call(
    addr: std::net::SocketAddr,
    method: Method,
    path: &str,
    cred: Cred<'_>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut req = client
        .request(method, format!("http://{addr}{path}"))
        .header("origin", PUBLIC_ORIGIN);
    req = match cred {
        Cred::Session(token) => req.header("cookie", format!("fvoci_session={token}")),
        Cred::Bearer(token) => req.header("authorization", format!("Bearer {token}")),
    };
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.expect("http");
    let status = response.status();
    let value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

async fn session_call(
    addr: std::net::SocketAddr,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(addr, method, path, Cred::Session(token), body).await
}

fn wiki_path(s: &SessionFixture, doc: Uuid, extra: &str) -> String {
    format!(
        "/api/v1/workspaces/{}/documents/{doc}{extra}",
        s.workspace_id
    )
}

fn project_path(s: &SessionFixture, project: Uuid, doc: Uuid, extra: &str) -> String {
    format!(
        "/api/v1/workspaces/{}/projects/{project}/documents/{doc}{extra}",
        s.workspace_id
    )
}

fn para(id: &str, text: &str) -> Value {
    json!({"type":"paragraph","attrs":{"id":id},"content":[{"type":"text","text":text}]})
}

fn doc_json(content: Vec<Value>) -> Value {
    json!({"type":"doc","content":content})
}

fn mention(entity: &str, id: Uuid) -> Value {
    json!({"type":"paragraph","content":[{"type":"mention","attrs":{"entity":entity,"id":id.to_string(),"label":"ref"}}]})
}

async fn admin_pool(harness: &TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap()
}

async fn count_updates(harness: &TestDb, workspace_id: Uuid, document_id: Uuid) -> i64 {
    let admin = admin_pool(harness).await;
    let n = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    n
}

async fn create_member_session(
    harness: &TestDb,
    workspace_id: Uuid,
    role: WorkspaceRole,
) -> SessionFixture {
    let admin = admin_pool(harness).await;
    let user_id = Uuid::now_v7();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("member-{user_id}@example.com"))
    .bind(&hash)
    .bind("Member")
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    workspace::add_membership_for_test(&pool, workspace_id, user_id, role)
        .await
        .unwrap();
    let token = new_token();
    let session_id = Uuid::now_v7();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    SessionFixture {
        pool,
        user_id,
        session_id,
        workspace_id,
        session_token: token.token,
    }
}

async fn api_token(s: &SessionFixture, scopes: &[ApiTokenScope]) -> String {
    create_api_token(
        &s.pool,
        s.workspace_id,
        s.user_id,
        s.session_id,
        CreateApiTokenInput {
            name: "doc api",
            scopes,
            unlimited: false,
            service: false,
        },
        None,
    )
    .await
    .expect("create token")
    .expect("created token")
    .token
}

async fn create_wiki_child(s: &SessionFixture, parent: Option<Uuid>, title: &str) -> Uuid {
    fvoci_server::db::documents::create_wiki_document(
        &s.pool,
        s.workspace_id,
        s.user_id,
        s.session_id,
        CreateDocumentInput {
            parent_id: parent,
            title,
            icon: None,
        },
        None,
    )
    .await
    .expect("create doc")
    .expect("created doc")
    .id
}

struct ProjectFixture {
    project_id: Uuid,
    root_document_id: Uuid,
    document_id: Uuid,
}

async fn create_project_with_doc(
    s: &SessionFixture,
    key: &str,
    visibility: &str,
) -> ProjectFixture {
    let project = fvoci_server::db::projects::create_project(
        &s.pool,
        s.workspace_id,
        s.user_id,
        s.session_id,
        fvoci_server::db::projects::CreateProjectInput {
            key,
            name: "문서 API 프로젝트",
            visibility,
            description: None,
            icon: None,
            lead_user_id: None,
        },
        None,
    )
    .await
    .expect("create project")
    .expect("created project");
    let root = project.root_document_id.expect("project root document");
    let created = fvoci_server::db::project_documents::create_project_document(
        &s.pool,
        s.workspace_id,
        project.id,
        s.user_id,
        s.session_id,
        CreateDocumentInput {
            parent_id: Some(root),
            title: "프로젝트 문서",
            icon: None,
        },
        None,
    )
    .await
    .expect("create project doc")
    .expect("created project doc");
    ProjectFixture {
        project_id: project.id,
        root_document_id: root,
        document_id: created.id,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn persist(ws: &mut Ws, key: &str) {
    let request_id = Uuid::now_v7();
    ws.send(Message::Binary(
        stateless_frame(key, &format!("persist:{request_id}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_stateless_exact(
            ws,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await,
        "persist ack"
    );
}

#[tokio::test]
async fn put_body_markdown_goes_through_live_room_and_survives_restart() {
    run_test("put_body_markdown_goes_through_live_room", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);

        let mut observer = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut observer, &key, 21).await;
        complete_sync_handshake(&mut observer, &key).await;
        let before = count_updates(&run.harness, s.workspace_id, wiki.document_id).await;

        let (status, meta) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            Some(json!({"contentMd": "# 제목\n\n안녕 본문 😀 한글"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        assert_eq!(meta["id"], wiki.document_id.to_string());
        assert!(
            wait_for_sync_update(&mut observer, Duration::from_secs(8)).await,
            "a live peer must receive the external write as a CRDT update"
        );
        assert_eq!(
            count_updates(&run.harness, s.workspace_id, wiki.document_id).await,
            before + 1,
            "the external write is exactly one durable collab update"
        );

        let (status, body) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let text = body["contentJson"].to_string();
        assert!(text.contains("제목") && text.contains("안녕 본문 😀 한글"), "{text}");
        assert_eq!(body["contentJson"]["content"][0]["type"], "heading");

        let (status, md) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, wiki.document_id, "/body?format=md"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{md}");
        let content_md = md["contentMd"].as_str().unwrap();
        // Source `documentContentMd` (TS `tiptapDocToMd`) output, byte for byte.
        assert_eq!(content_md, "# 제목\n\n안녕 본문 😀 한글\n");
        assert!(md.get("contentJson").is_none());
        let (status, _) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, wiki.document_id, "/body?format=html"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // A second full replace leaves only the new content (no append of old text).
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            Some(json!({"contentJson": doc_json(vec![para("p-1", "두 번째 본문")])})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        let live = support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id).await;
        let text = live["contentJson"].to_string();
        assert!(text.contains("두 번째 본문"), "{text}");
        assert!(!text.contains("안녕 본문"), "replace must remove the previous body {text}");
        assert_eq!(live["contentJson"]["content"][0]["attrs"]["id"], "p-1");

        let admin = admin_pool(&run.harness).await;
        let system_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND target_id = $2 AND verb = 'document.updated'",
        )
        .bind(s.workspace_id)
        .bind(wiki.document_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        assert!(system_events >= 2, "each projected write records document.updated");

        let _ = observer.close(None).await;
        run.shutdown_last_server().await.expect("stop");
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let durable = support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id).await;
        assert_eq!(durable["contentJson"], live["contentJson"]);
        let mut fresh = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut fresh, &key, 22).await;
        complete_sync_handshake(&mut fresh, &key).await;
        let _ = fresh.close(None).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

/// Rich Tiptap JSON already in schema-normal form (defaults present, no nulls,
/// full mark attrs, marks in name order) reads back unchanged: the Rust seed
/// round-trips through the room, the durable log and a fresh room.
#[tokio::test]
async fn put_body_rich_json_round_trips_through_rust_seed() {
    run_test("put_body_rich_json_round_trips_through_rust_seed", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let link = json!({"href": "https://x.example/?q=가", "target": "_blank",
            "rel": "noopener noreferrer nofollow", "class": null, "title": null});
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "attrs": {"id": "h-1", "level": 2},
             "content": [{"type": "text", "text": "제목 😀 𝒜 👩‍👩‍👧"}]},
            {"type": "paragraph", "attrs": {"id": "p-1", "textAlign": "center"}, "content": [
                {"type": "text", "text": "굵게", "marks": [{"type": "bold", "attrs": {}}]},
                {"type": "text", "text": " 링크", "marks": [
                    {"type": "italic", "attrs": {}}, {"type": "link", "attrs": link}]},
                {"type": "text", "text": " "},
                {"type": "mention", "attrs": {"entity": "user", "id": "u-1", "label": "김"}},
                {"type": "hardBreak"},
                {"type": "mathInline", "attrs": {"latex": "x^2"}},
                {"type": "text", "text": "끝", "marks": [
                    {"type": "highlight", "attrs": {"color": "#ff0"}}]}
            ]},
            {"type": "taskList", "attrs": {"id": "tl"}, "content": [
                {"type": "taskItem", "attrs": {"id": "ti", "checked": true}, "content": [
                    {"type": "paragraph", "attrs": {"id": "p-2"},
                     "content": [{"type": "text", "text": "완료"}]}]}]},
            {"type": "codeBlock", "attrs": {"id": "cb", "language": "rust", "highlightLines": [1]},
             "content": [{"type": "text", "text": "fn main() {}\n"}]},
            {"type": "table", "attrs": {"id": "t"}, "content": [{"type": "tableRow", "content": [
                {"type": "tableHeader", "attrs": {"colspan": 2, "rowspan": 1, "colwidth": [100, 50]},
                 "content": [{"type": "paragraph", "attrs": {"id": "p-3"},
                              "content": [{"type": "text", "text": "H"}]}]}]}]},
            {"type": "callout", "attrs": {"id": "c", "kind": "tip"},
             "content": [{"type": "paragraph", "attrs": {"id": "p-4"}}]},
            {"type": "attachment", "attrs": {"id": "a-1", "name": "사진.png", "image": true, "width": 320}},
            {"type": "embed", "attrs": {"id": "e-1", "entity": "url", "ref": "https://v.example"}}
        ]});
        let (status, meta) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            Some(json!({"contentJson": doc})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        let live =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        assert_eq!(live["contentJson"], doc);

        // A fresh server (new room loads the durable log) reads the same tree,
        // and a collab client completes the sync handshake on it.
        run.shutdown_last_server().await.expect("stop");
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);
        let mut client = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut client, &key, 31).await;
        complete_sync_handshake(&mut client, &key).await;
        let _ = client.close(None).await;
        let durable =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        assert_eq!(durable["contentJson"], doc);

        // Schema refusals from the seed (unknown node, empty text) stay 400.
        for bad in [
            json!({"type": "doc", "content": [{"type": "paragraph",
                   "content": [{"type": "text", "text": ""}]}]}),
            json!({"type": "doc", "content": [{"type": "paragraph",
                   "content": [{"type": "text", "text": "x", "marks": [{"type": "nope"}]}]}]}),
        ] {
            let (status, body) = session_call(
                addr,
                Method::PUT,
                &wiki_path(s, wiki.document_id, "/body"),
                &s.session_token,
                Some(json!({"contentJson": bad})),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        }
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn put_body_concurrent_with_live_edit_converges() {
    run_test("put_body_concurrent_with_live_edit_converges", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);

        let mut observer = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut observer, &key, 8).await;
        complete_sync_handshake(&mut observer, &key).await;
        let mut editor = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut editor, &key, 9).await;
        complete_sync_handshake(&mut editor, &key).await;

        // The PUT stops inside the room actor after its writer/auth step and
        // before it computes the forward update; a peer edit queues meanwhile.
        let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
        let put = tokio::spawn({
            let token = s.session_token.clone();
            let path = wiki_path(s, wiki.document_id, "/body");
            async move {
                session_call(
                    addr,
                    Method::PUT,
                    &path,
                    &token,
                    Some(json!({"contentMd": "외부 본문 교체"})),
                )
                .await
            }
        });
        reached.await.expect("PUT reached the actor barrier");
        editor
            .send(Message::Binary(
                sync_update_frame(&key, &engine_fixture("korean_emoji_base.v1")).into(),
            ))
            .await
            .unwrap();
        let _ = proceed.send(());
        let (status, body) = put.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            wait_for_sync_applied(&mut editor, Duration::from_secs(8)).await,
            "the concurrent peer edit must apply after the external write"
        );
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        persist(&mut editor, &key).await;

        let live =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        let text = live["contentJson"].to_string();
        assert!(
            text.contains("외부 본문 교체"),
            "external write kept {text}"
        );
        assert!(
            text.contains("가나다"),
            "concurrent peer edit must not be lost {text}"
        );

        let _ = editor.close(None).await;
        let _ = observer.close(None).await;
        run.shutdown_last_server().await.expect("stop");
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let durable =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        assert_eq!(
            durable["contentJson"], live["contentJson"],
            "durable == converged live state"
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn put_body_validation_permission_and_token_scopes() {
    run_test("put_body_validation_permission_and_token_scopes", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let path = wiki_path(s, wiki.document_id, "/body");

        for (body, code) in [
            (json!({}), StatusCode::BAD_REQUEST),
            (
                json!({"contentMd": "a", "contentJson": doc_json(vec![])}),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({"contentJson": {"type": "paragraph"}}),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({"contentJson": doc_json(vec![json!({"type":"noSuchNode"})])}),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({"contentMd": "가".repeat(400_000)}),
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            // Nested past the storable depth (Tiptap JSON > 126 levels):
            // refused by the Rust parser child, nothing is written.
            (
                json!({"contentMd": "> ".repeat(61) + "x"}),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (status, problem) =
                session_call(addr, Method::PUT, &path, &s.session_token, Some(body)).await;
            assert_eq!(status, code, "{problem}");
        }
        assert_eq!(
            count_updates(&run.harness, s.workspace_id, wiki.document_id).await,
            0
        );

        // Guests have no wiki access: the document does not exist for them.
        let guest = create_member_session(&run.harness, s.workspace_id, WorkspaceRole::Guest).await;
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &path,
            &guest.session_token,
            Some(json!({"contentMd": "침입"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let read_only = api_token(s, &[ApiTokenScope::DocumentsRead]).await;
        let (status, _) = call(
            addr,
            Method::PUT,
            &path,
            Cred::Bearer(&read_only),
            Some(json!({"contentMd": "읽기 토큰"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "documents.read cannot write");
        let (status, body) = call(
            addr,
            Method::GET,
            &format!("{path}?format=md"),
            Cred::Bearer(&read_only),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let tasks_only = api_token(s, &[ApiTokenScope::TasksWrite]).await;
        let (status, _) = call(addr, Method::GET, &path, Cred::Bearer(&tasks_only), None).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "tasks scope cannot read documents"
        );

        let writer = api_token(s, &[ApiTokenScope::DocumentsWrite]).await;
        let (status, meta) = call(
            addr,
            Method::PUT,
            &path,
            Cred::Bearer(&writer),
            Some(json!({"contentMd": "토큰으로 쓴 본문"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        let live =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        assert!(live["contentJson"].to_string().contains("토큰으로 쓴 본문"));
        assert_eq!(
            count_updates(&run.harness, s.workspace_id, wiki.document_id).await,
            1
        );

        // A revoked token stops writing on its next request.
        let admin = admin_pool(&run.harness).await;
        let token_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM fvoci.api_tokens WHERE user_id = $1 AND 'documents.write' = ANY(scopes)",
        )
        .bind(s.user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        fvoci_server::db::api_tokens::revoke_api_token(
            &s.pool,
            s.workspace_id,
            s.user_id,
            s.session_id,
            token_id,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let (status, _) = call(
            addr,
            Method::PUT,
            &path,
            Cred::Bearer(&writer),
            Some(json!({"contentMd": "철회 후"})),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn patch_block_replaces_one_block_through_the_room() {
    run_test("patch_block_replaces_one_block_through_the_room", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            Some(json!({"contentJson": doc_json(vec![para("blk-a", "첫 문단"), para("blk-b", "둘째 문단")])})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let mut observer = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut observer, &key, 31).await;
        complete_sync_handshake(&mut observer, &key).await;

        let (status, meta) = session_call(
            addr,
            Method::PATCH,
            &wiki_path(s, wiki.document_id, "/blocks/blk-b"),
            &s.session_token,
            Some(json!({"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"바뀐 제목 🎉"}]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        let live = support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id).await;
        let content = &live["contentJson"]["content"];
        assert_eq!(content[0]["attrs"]["id"], "blk-a");
        assert_eq!(content[0]["content"][0]["text"], "첫 문단");
        assert_eq!(content[1]["type"], "heading");
        assert_eq!(content[1]["attrs"]["id"], "blk-b");
        assert_eq!(content[1]["attrs"]["level"], 2);
        assert_eq!(content[1]["content"][0]["text"], "바뀐 제목 🎉");

        let (status, _) = session_call(
            addr,
            Method::PATCH,
            &wiki_path(s, wiki.document_id, "/blocks/no-such-block"),
            &s.session_token,
            Some(json!({"type":"paragraph"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, problem) = session_call(
            addr,
            Method::PATCH,
            &wiki_path(s, wiki.document_id, "/blocks/blk-a"),
            &s.session_token,
            Some(json!({"type":"paragraph","attrs":{"id":"other"}})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
        assert_eq!(problem["code"], "invalid_document_body");
        let _ = observer.close(None).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn project_document_body_block_children_ancestors_and_duplicate() {
    run_test("project_document_routes", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let owner = setup_owner_session(&run.harness).await;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let project = create_project_with_doc(&owner, "DOCAPI", "private").await;
        let doc = project.document_id;
        let child = fvoci_server::db::project_documents::create_project_document(
            &owner.pool,
            owner.workspace_id,
            project.project_id,
            owner.user_id,
            owner.session_id,
            CreateDocumentInput {
                parent_id: Some(doc),
                title: "하위 문서",
                icon: None,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id;

        let (status, meta) = session_call(
            addr,
            Method::PUT,
            &project_path(&owner, project.project_id, doc, "/body"),
            &owner.session_token,
            Some(json!({"contentJson": doc_json(vec![para("pb-1", "프로젝트 본문")])})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        assert_eq!(meta["projectId"], project.project_id.to_string());
        let (status, md) = session_call(
            addr,
            Method::GET,
            &project_path(&owner, project.project_id, doc, "/body?format=md"),
            &owner.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{md}");
        assert!(md["contentMd"].as_str().unwrap().contains("프로젝트 본문"));

        let (status, _) = session_call(
            addr,
            Method::PATCH,
            &project_path(&owner, project.project_id, doc, "/blocks/pb-1"),
            &owner.session_token,
            Some(json!({"type":"paragraph","content":[{"type":"text","text":"블록 수정"}]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Affiliation: the wiki route does not serve a project document.
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &wiki_path(&owner, doc, "/body"),
            &owner.session_token,
            Some(json!({"contentMd": "x"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, kids) = session_call(
            addr,
            Method::GET,
            &project_path(&owner, project.project_id, doc, "/children"),
            &owner.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = kids["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![child.to_string().as_str()]);

        let (status, crumbs) = session_call(
            addr,
            Method::GET,
            &project_path(&owner, project.project_id, child, "/ancestors"),
            &owner.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{crumbs}");
        let crumb_ids: Vec<&str> = crumbs["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            crumb_ids,
            vec![
                project.root_document_id.to_string().as_str(),
                doc.to_string().as_str()
            ]
        );

        // A workspace member outside the private project sees nothing.
        let outsider =
            create_member_session(&run.harness, owner.workspace_id, WorkspaceRole::Member).await;
        for extra in ["/children", "/ancestors", "/backlinks", "/body"] {
            let (status, _) = session_call(
                addr,
                Method::GET,
                &project_path(&owner, project.project_id, doc, extra),
                &outsider.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{extra}");
        }

        let (status, copy) = session_call(
            addr,
            Method::POST,
            &project_path(&owner, project.project_id, doc, "/duplicate"),
            &owner.session_token,
            Some(json!({"includeChildren": true})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{copy}");
        assert_eq!(copy["title"], "프로젝트 문서 (복사)");
        assert_eq!(copy["projectId"], project.project_id.to_string());
        assert!(copy["displayId"].as_str().unwrap().starts_with("DOCAPI-"));
        let copy_id: Uuid = copy["id"].as_str().unwrap().parse().unwrap();
        let (status, copy_body) = session_call(
            addr,
            Method::GET,
            &project_path(&owner, project.project_id, copy_id, "/body"),
            &owner.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(copy_body["contentJson"].to_string().contains("블록 수정"));
        let (_, copy_kids) = session_call(
            addr,
            Method::GET,
            &project_path(&owner, project.project_id, copy_id, "/children"),
            &owner.session_token,
            None,
        )
        .await;
        assert_eq!(copy_kids["items"].as_array().unwrap().len(), 1);
        assert_eq!(copy_kids["items"][0]["title"], "하위 문서");
        run.finish().await.expect("cleanup");
    })
    .await;
}

async fn count_documents(harness: &TestDb, workspace_id: Uuid) -> i64 {
    let admin = admin_pool(harness).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
        .bind(workspace_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    n
}

/// Review B1: a wiki group editor grant does not confer workspace wiki-create
/// rights, so a guest cannot duplicate (create) wiki documents.
#[tokio::test]
async fn duplicate_requires_workspace_wiki_edit_not_a_group_grant() {
    run_test("duplicate_requires_wiki_edit", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let guest = create_member_session(&run.harness, s.workspace_id, WorkspaceRole::Guest).await;
        let admin = admin_pool(&run.harness).await;
        let group_id = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.groups (id, workspace_id, name) VALUES ($1, $2, 'wiki-editors')")
            .bind(group_id)
            .bind(s.workspace_id)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO fvoci.group_members (workspace_id, group_id, user_id) VALUES ($1, $2, $3)",
        )
        .bind(s.workspace_id)
        .bind(group_id)
        .bind(guest.user_id)
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role) VALUES ($1, $2, $3, $4, 'member')",
        )
        .bind(Uuid::now_v7())
        .bind(s.workspace_id)
        .bind(wiki.document_id)
        .bind(group_id)
        .execute(&admin)
        .await
        .unwrap();
        admin.close().await;

        // The grant does allow editing the body itself.
        let (status, meta) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &guest.session_token,
            Some(json!({"contentMd": "게스트 편집"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");

        let before = count_documents(&run.harness, s.workspace_id).await;
        for path in [
            wiki_path(s, wiki.document_id, "/duplicate"),
            format!("/api/v1/documents/{}/duplicate", wiki.document_id),
        ] {
            let (status, body) = session_call(
                addr,
                Method::POST,
                &path,
                &guest.session_token,
                Some(json!({"includeChildren": true})),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        }
        assert_eq!(count_documents(&run.harness, s.workspace_id).await, before);
        run.finish().await.expect("cleanup");
    })
    .await;
}

/// Review B2: the project root has no parent; a copy would be a second
/// parentless project document, which the source refuses (affiliation).
#[tokio::test]
async fn duplicate_of_project_root_is_an_affiliation_mismatch() {
    run_test("duplicate_project_root", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let owner = setup_owner_session(&run.harness).await;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let project = create_project_with_doc(&owner, "DOCROOT", "private").await;
        let before = count_documents(&run.harness, owner.workspace_id).await;
        for include_children in [false, true] {
            let (status, problem) = session_call(
                addr,
                Method::POST,
                &project_path(
                    &owner,
                    project.project_id,
                    project.root_document_id,
                    "/duplicate",
                ),
                &owner.session_token,
                Some(json!({"includeChildren": include_children})),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
        }
        assert_eq!(
            count_documents(&run.harness, owner.workspace_id).await,
            before
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn duplicate_copies_live_state_into_an_independent_room() {
    run_test("duplicate_copies_live_state", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);

        // The source body lives only in collab state (a peer edit).
        let mut editor = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut editor, &key, 41).await;
        complete_sync_handshake(&mut editor, &key).await;
        editor
            .send(Message::Binary(
                sync_update_frame(&key, &engine_fixture("korean_emoji_base.v1")).into(),
            ))
            .await
            .unwrap();
        assert!(wait_for_sync_applied(&mut editor, Duration::from_secs(8)).await);
        persist(&mut editor, &key).await;

        let admin = admin_pool(&run.harness).await;
        let tag_id = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.document_tags (id, workspace_id, name, color) VALUES ($1, $2, '복제태그', 'blue')")
            .bind(tag_id)
            .bind(s.workspace_id)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fvoci.document_tag_assignments (workspace_id, document_id, tag_id) VALUES ($1, $2, $3)")
            .bind(s.workspace_id)
            .bind(wiki.document_id)
            .bind(tag_id)
            .execute(&admin)
            .await
            .unwrap();
        let sibling = create_wiki_child(s, None, "다음 형제").await;
        let child = create_wiki_child(s, Some(wiki.document_id), "자식").await;

        let (status, copy) = session_call(
            addr,
            Method::POST,
            &wiki_path(s, wiki.document_id, "/duplicate"),
            &s.session_token,
            Some(json!({"title": "  사본 제목  "})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{copy}");
        assert_eq!(copy["title"], "사본 제목");
        assert!(copy["displayId"].as_str().unwrap().starts_with("WIKI-"));
        let copy_id: Uuid = copy["id"].as_str().unwrap().parse().unwrap();

        let source_body = support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id).await;
        let copy_body = support::get_document_body(addr, &s.session_token, s.workspace_id, copy_id).await;
        assert!(copy_body["contentJson"].to_string().contains("가나다"));
        assert_eq!(copy_body["contentJson"], source_body["contentJson"]);

        // Placement: right after the source, before its next sibling; no children copied.
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT id, sort_key FROM fvoci.documents WHERE workspace_id = $1 AND parent_id IS NULL AND deleted_at IS NULL ORDER BY sort_key COLLATE \"C\"",
        )
        .bind(s.workspace_id)
        .fetch_all(&admin)
        .await
        .unwrap();
        let order: Vec<Uuid> = rows.into_iter().map(|(id, _)| id).collect();
        assert_eq!(order, vec![wiki.document_id, copy_id, sibling]);
        let copied_children: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1 AND parent_id = $2",
        )
        .bind(s.workspace_id)
        .bind(copy_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(copied_children, 0, "includeChildren omitted copies no children ({child})");
        let tags: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.document_tag_assignments WHERE workspace_id = $1 AND document_id = $2 AND tag_id = $3",
        )
        .bind(s.workspace_id)
        .bind(copy_id)
        .bind(tag_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(tags, 1);
        let (state_rows, tail): (i64, i64) = sqlx::query_as(
            "SELECT count(*), coalesce(max(tail_seq), -1) FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2",
        )
        .bind(s.workspace_id)
        .bind(copy_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!((state_rows, tail), (1, 0), "copy has its own seeded collab state");
        let created_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.audit_log WHERE workspace_id = $1 AND target_id = $2 AND verb = 'document.created'",
        )
        .bind(s.workspace_id)
        .bind(copy_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(created_events, 1);
        admin.close().await;

        // The copy opens as its own room and edits there do not touch the source.
        let copy_key = routing_key(s.workspace_id, copy_id);
        let mut copy_editor = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut copy_editor, &copy_key, 42).await;
        complete_sync_handshake(&mut copy_editor, &copy_key).await;
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, copy_id, "/body"),
            &s.session_token,
            Some(json!({"contentMd": "사본만 수정"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(wait_for_sync_update(&mut copy_editor, Duration::from_secs(8)).await);
        let source_after = support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id).await;
        assert_eq!(source_after["contentJson"], source_body["contentJson"]);

        // Flat routes resolve the workspace from the document and are session only.
        let (status, flat) = session_call(
            addr,
            Method::GET,
            &format!("/api/v1/documents/{copy_id}"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(flat["title"], "사본 제목");
        let (status, flat) = session_call(
            addr,
            Method::PATCH,
            &format!("/api/v1/documents/{copy_id}"),
            &s.session_token,
            Some(json!({"title":"평면 수정"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{flat}");
        assert_eq!(flat["title"], "평면 수정");
        let (status, dup) = session_call(
            addr,
            Method::POST,
            &format!("/api/v1/documents/{copy_id}/duplicate"),
            &s.session_token,
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{dup}");
        assert_eq!(dup["title"], "평면 수정 (복사)");
        let token = api_token(s, &[ApiTokenScope::DocumentsWrite]).await;
        let (status, _) = call(
            addr,
            Method::GET,
            &format!("/api/v1/documents/{copy_id}"),
            Cred::Bearer(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "flat routes are session only");
        let (status, _) = session_call(
            addr,
            Method::DELETE,
            &format!("/api/v1/documents/{copy_id}"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = session_call(
            addr,
            Method::GET,
            &format!("/api/v1/documents/{copy_id}"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let stranger = setup_owner_session(&run.harness).await;
        let (status, _) = session_call(
            addr,
            Method::GET,
            &format!("/api/v1/documents/{}", wiki.document_id),
            &stranger.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "another workspace's document is not found");

        let _ = copy_editor.close(None).await;
        let _ = editor.close(None).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn backlinks_follow_body_references_and_reader_permissions() {
    run_test("backlinks", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki: WikiDocFixture = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let target = wiki.document_id;
        let wiki_ref = create_wiki_child(s, None, "위키 참조").await;
        let unrelated = create_wiki_child(s, None, "무관").await;
        let child = create_wiki_child(s, Some(target), "자식 문서").await;
        let project = create_project_with_doc(s, "BLNK", "private").await;

        for (path, body) in [
            (
                wiki_path(s, wiki_ref, "/body"),
                doc_json(vec![mention("document", target)]),
            ),
            (
                wiki_path(s, unrelated, "/body"),
                doc_json(vec![mention("user", target), para("u", &target.to_string())]),
            ),
            (
                project_path(s, project.project_id, project.document_id, "/body"),
                doc_json(vec![json!({"type":"embed","attrs":{"entity":"document","ref":target.to_string().to_uppercase()}})]),
            ),
            (
                wiki_path(s, target, "/body"),
                doc_json(vec![mention("document", target)]),
            ),
        ] {
            let (status, meta) =
                session_call(addr, Method::PUT, &path, &s.session_token, Some(json!({"contentJson": body}))).await;
            assert_eq!(status, StatusCode::OK, "{path} {meta}");
        }
        // Task bodies have no product write route yet: the fixture sets one directly.
        let task = fvoci_server::db::tasks::create_task(
            &s.pool,
            s.workspace_id,
            project.project_id,
            s.user_id,
            s.session_id,
            fvoci_server::db::tasks::CreateTaskInput {
                title: "참조 태스크",
                task_type: "task",
                priority: "none",
                status_id: None,
                start_date: None,
                due_date: None,
                parent_id: None,
                milestone_id: None,
                recurrence: None,
            },
            None,
            "web",
        )
        .await
        .unwrap()
        .unwrap();
        let admin = admin_pool(&run.harness).await;
        sqlx::query("UPDATE fvoci.tasks SET content_json = $3 WHERE workspace_id = $1 AND id = $2")
            .bind(s.workspace_id)
            .bind(task.id)
            .bind(doc_json(vec![mention("document", target)]))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;

        let (status, links) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, target, "/backlinks"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{links}");
        let items = links["items"].as_array().unwrap();
        let from: Vec<(String, String)> = items
            .iter()
            .map(|i| (i["from"]["type"].as_str().unwrap().to_string(), i["from"]["id"].as_str().unwrap().to_string()))
            .collect();
        let mut expected = vec![
            ("document".to_string(), wiki_ref.to_string()),
            ("document".to_string(), project.document_id.to_string()),
            ("task".to_string(), task.id.to_string()),
        ];
        expected.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(from, expected, "self and non-document mentions are not backlinks");
        let project_item = items.iter().find(|i| i["from"]["id"] == project.document_id.to_string()).unwrap();
        assert!(project_item["from"]["displayId"].as_str().unwrap().starts_with("BLNK-"));
        let wiki_item = items.iter().find(|i| i["from"]["id"] == wiki_ref.to_string()).unwrap();
        assert!(wiki_item["from"]["displayId"].as_str().unwrap().starts_with("WIKI-"));

        // A member outside the private project does not see its referencing document.
        let member = create_member_session(&run.harness, s.workspace_id, WorkspaceRole::Member).await;
        let (status, links) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, target, "/backlinks"),
            &member.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = links["items"].as_array().unwrap().iter().map(|i| i["from"]["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec![wiki_ref.to_string().as_str()]);

        // Trashing the referencing document removes the backlink.
        let (status, _) = session_call(
            addr,
            Method::DELETE,
            &wiki_path(s, wiki_ref, ""),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, links) = session_call(
            addr,
            Method::GET,
            &wiki_path(s, target, "/backlinks"),
            &s.session_token,
            None,
        )
        .await;
        let ids: Vec<&str> = links["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["from"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 2, "{links}");
        assert!(!ids.contains(&wiki_ref.to_string().as_str()));

        // Children of a wiki document, and read-scoped tokens.
        let reader = api_token(s, &[ApiTokenScope::DocumentsRead]).await;
        let (status, kids) = call(
            addr,
            Method::GET,
            &wiki_path(s, target, "/children"),
            Cred::Bearer(&reader),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(kids["items"][0]["id"], child.to_string());
        assert_eq!(kids["items"].as_array().unwrap().len(), 1);
        let tasks = api_token(s, &[ApiTokenScope::TasksRead]).await;
        let (status, _) = call(
            addr,
            Method::GET,
            &wiki_path(s, target, "/backlinks"),
            Cred::Bearer(&tasks),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn put_body_rejected_when_access_revoked_inside_the_room() {
    run_test("put_body_rejected_when_access_revoked", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let s = &wiki.session;
        let member =
            create_member_session(&run.harness, s.workspace_id, WorkspaceRole::Member).await;
        let (state, hub) = app_state(&run.harness).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(s.workspace_id, wiki.document_id);
        let mut observer = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut observer, &key, 51).await;
        complete_sync_handshake(&mut observer, &key).await;

        // The HTTP precheck passes; membership is removed while the room actor
        // holds the write between its writer step and its locked recheck.
        let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
        let put = tokio::spawn({
            let token = member.session_token.clone();
            let path = wiki_path(s, wiki.document_id, "/body");
            async move {
                session_call(
                    addr,
                    Method::PUT,
                    &path,
                    &token,
                    Some(json!({"contentMd": "철회된 쓰기"})),
                )
                .await
            }
        });
        reached.await.expect("PUT reached the actor barrier");
        workspace::remove_member(
            &s.pool,
            s.workspace_id,
            s.user_id,
            s.session_id,
            member.user_id,
            None,
        )
        .await
        .unwrap()
        .expect("removed member");
        let _ = proceed.send(());
        let (status, body) = put.await.unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(
            count_updates(&run.harness, s.workspace_id, wiki.document_id).await,
            0
        );
        let live =
            support::get_document_body(addr, &s.session_token, s.workspace_id, wiki.document_id)
                .await;
        assert!(!live["contentJson"].to_string().contains("철회된 쓰기"));

        // The room stays usable for remaining members.
        let (status, _) = session_call(
            addr,
            Method::PUT,
            &wiki_path(s, wiki.document_id, "/body"),
            &s.session_token,
            Some(json!({"contentMd": "소유자 쓰기"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        let _ = observer.close(None).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}
