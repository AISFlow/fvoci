#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Semantic (hybrid) search end to end: index-time chunk embeddings stored in
//! PostgreSQL and copied into the real Meili CE test server, workspace
//! `mode=hybrid` ranking, ACL via PG hydrate, and the lexical fallbacks.
//! The only fake is the embeddings provider: a local OpenAI-compatible server
//! on 127.0.0.1:0 returning deterministic topic vectors.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use fvoci_server::db::attachment_extract::{finish_extract, ExtractClaim, FinishExtract};
use fvoci_server::db::search_index::{store_chunk_embeddings, PendingEmbeddingChunk};
use fvoci_server::search::embed::{Embedder, EmbedderEnv};
use fvoci_server::search::embed_pass::{run_embed_pass, EmbedBackoff, EmbedPassOutcome};
use fvoci_server::search::index::{process_search_index_event, rebuild_search_index};
use fvoci_server::search::meili::{
    delete_all_meili_documents, ensure_meili_index, search_source_id, MeiliConfig, SearchSourceKind,
};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, app_state, create_project, insert_project_document,
    insert_stored_attachment, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SECRET: &str = "sk-fake-semantic-secret-0123456789";
const MODEL: &str = "fake-embed-1";
const DIM: usize = 1536;

/// Topic words -> one axis each. Texts sharing a topic have cosine ~1,
/// unrelated texts ~0 (the shared bias axis keeps every vector non-zero).
const TOPICS: &[&[&str]] = &[
    &["고양이", "cat", "kitten", "feline"],
    &["자동차", "car", "vehicle"],
    &["우주", "로켓", "rocket", "space"],
];

fn topic_vector(text: &str) -> Vec<f32> {
    let lower = text.to_lowercase();
    let mut v = vec![0f32; DIM];
    for (axis, words) in TOPICS.iter().enumerate() {
        if words.iter().any(|w| lower.contains(w)) {
            v[axis * 7] = 1.0;
        }
    }
    v[DIM - 1] = 0.05;
    v
}

const MODE_OK: u8 = 0;
const MODE_HTTP_500: u8 = 1;
const MODE_WRONG_DIM: u8 = 2;

#[derive(Debug, Clone)]
struct SeenCall {
    authorization: Option<String>,
    model: Option<String>,
    inputs: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeState {
    mode: Arc<AtomicU8>,
    calls: Arc<Mutex<Vec<SeenCall>>>,
}

struct FakeEmbedder {
    addr: SocketAddr,
    state: FakeState,
    _task: tokio::task::JoinHandle<()>,
}

impl FakeEmbedder {
    async fn spawn() -> Self {
        async fn embeddings(
            State(state): State<FakeState>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let inputs: Vec<String> = body["input"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            state.calls.lock().unwrap().push(SeenCall {
                authorization: headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
                model: body["model"].as_str().map(str::to_string),
                inputs: inputs.clone(),
            });
            match state.mode.load(Ordering::SeqCst) {
                MODE_HTTP_500 => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "provider down"})),
                ),
                MODE_WRONG_DIM => (
                    StatusCode::OK,
                    Json(
                        json!({"data": inputs.iter().enumerate().map(|(i, _)| json!({"index": i, "embedding": [1.0, 2.0]})).collect::<Vec<_>>()}),
                    ),
                ),
                _ => {
                    // Reverse order with explicit indices: the client must sort.
                    let data: Vec<Value> = inputs
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(i, text)| json!({"index": i, "embedding": topic_vector(text)}))
                        .collect();
                    (StatusCode::OK, Json(json!({"data": data, "model": MODEL})))
                }
            }
        }
        let state = FakeState::default();
        let app = Router::new()
            .route("/v1/embeddings", post(embeddings))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake embedder");
        let addr = listener.local_addr().expect("addr");
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("fake embedder");
        });
        Self {
            addr,
            state,
            _task: task,
        }
    }

    fn embedder(&self) -> Embedder {
        embedder_for(&format!("http://{}/v1/", self.addr))
    }

    fn set_mode(&self, mode: u8) {
        self.state.mode.store(mode, Ordering::SeqCst);
    }

    fn calls(&self) -> Vec<SeenCall> {
        self.state.calls.lock().unwrap().clone()
    }
}

fn embedder_for(base_url: &str) -> Embedder {
    Embedder::from_values(EmbedderEnv {
        enabled: Some("1"),
        secret: Some(SECRET),
        base_url: Some(base_url),
        model: Some(MODEL),
        dim: Some("1536"),
        allow_private: Some("1"),
    })
    .expect("embedder config")
    .expect("embedder enabled")
}

fn test_meili() -> MeiliConfig {
    let url = std::env::var("FVOCI_MEILI_URL").expect("FVOCI_MEILI_URL");
    let key = std::env::var("FVOCI_MEILI_KEY").expect("FVOCI_MEILI_KEY");
    MeiliConfig::new(url, key, format!("fvoci_{}", Uuid::now_v7().simple()))
}

fn router(state: fvoci_server::http::state::AppState) -> Router {
    fvoci_server::http::router(state, None)
}

async fn search_app(harness: &TestDb, meili: &MeiliConfig, embedder: Option<Embedder>) -> Router {
    let mut state = app_state(&harness.app_url).await;
    state.meili = Some(meili.clone());
    state.search_embedder = embedder;
    router(state)
}

async fn insert_wiki_document(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    created_by: Uuid,
    number: i32,
    title: &str,
    body: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by, text
        ) VALUES (
            $1, $2, $3, $4, NULL, 'V', NULL, $5, 'published', 2,
            '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $6, $7
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(title)
    .bind(document_id.simple().to_string())
    .bind(number)
    .bind(created_by)
    .bind(body)
    .execute(admin)
    .await
    .expect("insert wiki document");
}

/// Product extraction finish (chunks + `attachment.extracted`) under a lease.
async fn extract_text(admin: &PgPool, app: &PgPool, workspace_id: Uuid, id: Uuid, text: &str) {
    let lease = Uuid::now_v7();
    sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_lease_token = $2,
            extract_lease_expires_at = now() + interval '5 minutes',
            extract_status = 'pending'
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(lease)
    .execute(admin)
    .await
    .expect("lease");
    let applied = finish_extract(
        app,
        &ExtractClaim {
            workspace_id,
            attachment_id: id,
            lease_token: lease,
            attempt: 1,
        },
        &FinishExtract {
            status: "ok".into(),
            text: text.to_string(),
            warnings: Vec::new(),
            rhwp_rev: None,
        },
    )
    .await
    .expect("finish extract");
    assert!(applied);
}

/// Delivers every search-relevant event not yet delivered by this helper,
/// in id order, through the product consumer.
async fn deliver_events(
    admin: &PgPool,
    app: &PgPool,
    meili: &MeiliConfig,
    delivered: &mut Vec<Uuid>,
) -> usize {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM fvoci.events WHERE verb LIKE 'attachment.%' ORDER BY id",
    )
    .fetch_all(admin)
    .await
    .expect("events");
    let mut count = 0;
    for id in ids {
        if delivered.contains(&id) {
            continue;
        }
        let event = fvoci_server::db::outbox::fetch_event_by_id(admin, id)
            .await
            .expect("fetch")
            .expect("row");
        process_search_index_event(app, meili, &event)
            .await
            .expect("index event");
        delivered.push(id);
        count += 1;
    }
    count
}

async fn embed_all(pool: &PgPool, embedder: &Embedder) -> Vec<Uuid> {
    let mut state = EmbedBackoff::default();
    let cancel = CancellationToken::new();
    let mut done = Vec::new();
    loop {
        match run_embed_pass(pool, embedder, &mut state, &cancel)
            .await
            .expect("embed pass")
        {
            EmbedPassOutcome::Embedded { attachment_id, .. } => done.push(attachment_id),
            EmbedPassOutcome::Idle => return done,
            other => panic!("unexpected embed pass outcome {other:?}"),
        }
    }
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn search(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    q: &str,
    extra: &str,
) -> (StatusCode, Value) {
    let path = format!(
        "/api/v1/workspaces/{workspace_id}/search?q={}{}",
        urlencoding(q),
        if extra.is_empty() {
            String::new()
        } else {
            format!("&{extra}")
        }
    );
    json_request(app.clone(), "GET", &path, None, Some(cookie)).await
}

fn ids_of(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|item| item["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

async fn chunk_embedding_counts(admin: &PgPool, attachment_id: Uuid) -> (i64, i64) {
    sqlx::query_as(
        r#"
        SELECT count(*) FILTER (WHERE embedding IS NOT NULL),
               count(*) FILTER (WHERE embedding IS NULL)
        FROM fvoci.attachment_text WHERE attachment_id = $1
        "#,
    )
    .bind(attachment_id)
    .fetch_one(admin)
    .await
    .expect("embedding counts")
}

async fn meili_vectors(meili: &MeiliConfig, id: &str) -> Value {
    let url = format!(
        "{}/indexes/{}/documents/{}?retrieveVectors=true",
        meili.url, meili.index_uid, id
    );
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(meili.api_key())
        .send()
        .await
        .expect("get document");
    assert_eq!(resp.status().as_u16(), 200, "get document {id}");
    let doc: Value = resp.json().await.expect("json");
    doc["_vectors"]["attachments"].clone()
}

fn cat_story(extra: &str) -> String {
    format!("우리 집 고양이는 창가에서 낮잠을 잔다. {extra}\n\n털이 부드럽고 조용하다.")
}

#[tokio::test]
async fn hybrid_ranks_embedded_attachment_chunks_and_keeps_acl() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure index");
    let fake = FakeEmbedder::spawn().await;
    let embedder = fake.embedder();
    let app = search_app(&harness, &meili, Some(embedder.clone())).await;

    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let wiki_id = Uuid::now_v7();
    insert_wiki_document(
        &admin,
        workspace_id,
        wiki_id,
        owner_id,
        101,
        "노트",
        "첨부 모음",
    )
    .await;
    let private = create_project(app.clone(), &owner_cookie, workspace_id, "PRIV", "private").await;
    let private_id = Uuid::parse_str(private["id"].as_str().unwrap()).unwrap();
    let private_doc = Uuid::now_v7();
    insert_project_document(&admin, workspace_id, private_id, private_doc, owner_id, 5).await;

    let cat_wiki = insert_stored_attachment(&admin, workspace_id, wiki_id, owner_id).await;
    let car_wiki = insert_stored_attachment(&admin, workspace_id, wiki_id, owner_id).await;
    let cat_private = insert_stored_attachment(&admin, workspace_id, private_doc, owner_id).await;
    let long_cat = format!(
        "{}\n\n{}",
        "고양이 산책 기록 ".repeat(150),
        "마지막 문단 ".repeat(150)
    );
    extract_text(&admin, &pool, workspace_id, cat_wiki, &long_cat).await;
    extract_text(
        &admin,
        &pool,
        workspace_id,
        car_wiki,
        "새 자동차 정비 일지.",
    )
    .await;
    extract_text(
        &admin,
        &pool,
        workspace_id,
        cat_private,
        &cat_story("비공개"),
    )
    .await;
    let mut delivered = Vec::new();
    assert_eq!(
        deliver_events(&admin, &pool, &meili, &mut delivered).await,
        3
    );

    // Chunks are indexed lexically before any vector exists.
    let (with, without) = chunk_embedding_counts(&admin, cat_wiki).await;
    assert_eq!(with, 0);
    assert!(without >= 2, "long text is chunked: {without}");
    let (status, body) = search(&app, &owner_cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(ids_of(&body).is_empty(), "no vectors yet: {body}");

    let mut embedded = embed_all(&pool, &embedder).await;
    embedded.sort();
    let mut expected = vec![cat_wiki, car_wiki, cat_private];
    expected.sort();
    assert_eq!(embedded, expected);
    for id in [cat_wiki, car_wiki, cat_private] {
        let (with, without) = chunk_embedding_counts(&admin, id).await;
        assert!(with >= 1, "{id} has vectors");
        assert_eq!(without, 0, "{id} fully embedded");
    }
    let calls_after_index = fake.calls();
    assert!(!calls_after_index.is_empty());
    for call in &calls_after_index {
        assert_eq!(
            call.authorization.as_deref(),
            Some(format!("Bearer {SECRET}").as_str())
        );
        assert_eq!(call.model.as_deref(), Some(MODEL));
        assert!(call.inputs.len() <= 32, "source batch size");
    }
    let embedded_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.embedded'")
            .fetch_one(&admin)
            .await
            .expect("count events");
    assert_eq!(embedded_events, 3);
    assert_eq!(
        deliver_events(&admin, &pool, &meili, &mut delivered).await,
        3
    );
    let vectors = meili_vectors(
        &meili,
        &search_source_id(SearchSourceKind::Attachment, &cat_wiki.to_string(), Some(0)),
    )
    .await;
    assert_eq!(
        vectors["embeddings"][0].as_array().map(Vec::len),
        Some(DIM),
        "{vectors}"
    );
    // Nothing left to embed: the pass is idle and makes no provider call.
    let calls = fake.calls().len();
    assert!(embed_all(&pool, &embedder).await.is_empty());
    assert_eq!(fake.calls().len(), calls);

    // Lexical never matches "cat"; hybrid finds the semantically close chunks.
    let (status, body) = search(&app, &owner_cookie, workspace_id, "cat", "mode=lexical").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids_of(&body).is_empty(), "{body}");
    let calls = fake.calls().len();
    let (status, body) = search(&app, &owner_cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fake.calls().len(), calls + 1, "one query embedding");
    assert_eq!(fake.calls().last().unwrap().inputs, vec!["cat".to_string()]);
    let ids = ids_of(&body);
    assert_eq!(ids.len(), 2, "{body}");
    assert!(ids.contains(&cat_wiki.to_string()));
    assert!(ids.contains(&cat_private.to_string()));
    assert!(
        !ids.contains(&car_wiki.to_string()),
        "unrelated vector is below the floor"
    );
    for item in body["items"].as_array().unwrap() {
        assert_eq!(item["type"], "attachment");
        assert!(
            item["chunkNo"].is_null(),
            "semantic-only hits have no chunk: {item}"
        );
        let score = item["score"].as_f64().unwrap();
        assert!((score - 1.0 / 61.0).abs() < 1e-9 || (score - 1.0 / 62.0).abs() < 1e-9);
    }
    let (_, only_attachments) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "cat",
        "mode=hybrid&type=attachment",
    )
    .await;
    assert_eq!(ids_of(&only_attachments).len(), 2);
    let (_, documents) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "cat",
        "mode=hybrid&type=document",
    )
    .await;
    assert!(
        ids_of(&documents).is_empty(),
        "no semantic pass for documents"
    );

    // Both lists: the lexical chunk hit keeps its chunk number and rises to the top.
    let (_, both) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "고양이 산책",
        "mode=hybrid",
    )
    .await;
    let first = &both["items"][0];
    assert_eq!(first["id"], cat_wiki.to_string(), "{both}");
    assert!(first["chunkNo"].is_number(), "{first}");
    // Lexical rank 0 plus a semantic rank (0 or 1: the two cat chunks tie on cosine).
    assert!(
        first["score"].as_f64().unwrap() > 1.0 / 61.0 + 1.0 / 63.0,
        "{first}"
    );

    // PG hydrate stays the boundary: a member without the private project.
    let (status, body) = search(&app, &member.cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids_of(&body), vec![cat_wiki.to_string()]);

    // A non-member gets 404 before any provider call.
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE user_id = $1")
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .expect("drop membership");
    let calls = fake.calls().len();
    let (status, _) = search(&app, &outsider.cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(fake.calls().len(), calls);

    // Global search ignores hybrid and never embeds.
    let (status, body) = json_request(
        app.clone(),
        "GET",
        "/api/v1/search?q=cat&mode=hybrid",
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(ids_of(&body).is_empty(), "{body}");
    assert_eq!(fake.calls().len(), calls);

    // Offset pages over one fused pool; the cursor is bound to hybrid.
    let (_, page1) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "cat",
        "mode=hybrid&limit=1",
    )
    .await;
    assert_eq!(ids_of(&page1).len(), 1);
    let cursor = page1["nextCursor"]
        .as_str()
        .expect("next cursor")
        .to_string();
    let (status, page2) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "cat",
        &format!("mode=hybrid&limit=1&cursor={}", urlencoding(&cursor)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(ids_of(&page2).len(), 1);
    assert_ne!(ids_of(&page1), ids_of(&page2));
    assert!(page2["nextCursor"].is_null(), "{page2}");
    let (status, _) = search(
        &app,
        &owner_cookie,
        workspace_id,
        "cat",
        &format!("mode=lexical&limit=1&cursor={}", urlencoding(&cursor)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "hybrid cursor is not a lexical cursor"
    );

    // Rebuild from PG restores the vectors without calling the provider.
    delete_all_meili_documents(&meili)
        .await
        .expect("wipe index");
    let calls = fake.calls().len();
    rebuild_search_index(&pool, &meili, Some(workspace_id))
        .await
        .expect("rebuild");
    assert_eq!(fake.calls().len(), calls);
    let (_, rebuilt) = search(&app, &owner_cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(ids_of(&rebuilt).len(), 2, "{rebuilt}");

    // Re-extraction replaces the chunks: stale vectors never survive, a late
    // write for the old text is refused, and the new text is embedded again.
    extract_text(
        &admin,
        &pool,
        workspace_id,
        cat_wiki,
        "로켓 발사 준비 체크리스트",
    )
    .await;
    let stale = store_chunk_embeddings(
        &pool,
        workspace_id,
        cat_wiki,
        &[(
            PendingEmbeddingChunk {
                chunk_no: 0,
                text: long_cat.chars().take(10).collect(),
            },
            topic_vector("cat"),
        )],
    )
    .await
    .expect("stale store");
    assert_eq!(stale, 0);
    assert_eq!(chunk_embedding_counts(&admin, cat_wiki).await, (0, 1));
    assert_eq!(embed_all(&pool, &embedder).await, vec![cat_wiki]);
    deliver_events(&admin, &pool, &meili, &mut delivered).await;
    let (_, after) = search(&app, &owner_cookie, workspace_id, "cat", "mode=hybrid").await;
    assert_eq!(ids_of(&after), vec![cat_private.to_string()]);
    let (_, rocket) = search(&app, &owner_cookie, workspace_id, "space", "mode=hybrid").await;
    assert_eq!(ids_of(&rocket), vec![cat_wiki.to_string()]);

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn provider_failures_fall_back_to_lexical_and_retry_later() {
    let harness = TestDb::bootstrap().await;
    let (_, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure index");
    let fake = FakeEmbedder::spawn().await;
    let embedder = fake.embedder();
    let app = search_app(&harness, &meili, Some(embedder.clone())).await;

    let token = format!("qsem{}", Uuid::now_v7().simple());
    let wiki_id = Uuid::now_v7();
    insert_wiki_document(
        &admin,
        workspace_id,
        wiki_id,
        owner_id,
        101,
        &format!("{token} 문서"),
        "본문",
    )
    .await;
    let attachment = insert_stored_attachment(&admin, workspace_id, wiki_id, owner_id).await;
    extract_text(&admin, &pool, workspace_id, attachment, &cat_story(&token)).await;
    let mut delivered = Vec::new();
    deliver_events(&admin, &pool, &meili, &mut delivered).await;
    let doc_event: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, payload, channel)
        VALUES ($1, $2, 'document.created', 'document', $3, '{}'::jsonb, 'system')
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(wiki_id)
    .fetch_one(&admin)
    .await
    .expect("doc event");
    let event = fvoci_server::db::outbox::fetch_event_by_id(&admin, doc_event)
        .await
        .expect("fetch")
        .expect("row");
    process_search_index_event(&pool, &meili, &event)
        .await
        .expect("index doc");

    // Provider answers 500: nothing stored, no event, and the pass backs off
    // instead of calling again right away.
    fake.set_mode(MODE_HTTP_500);
    let cancel = CancellationToken::new();
    let mut backoff = EmbedBackoff::default();
    assert_eq!(
        run_embed_pass(&pool, &embedder, &mut backoff, &cancel)
            .await
            .unwrap(),
        EmbedPassOutcome::Waiting
    );
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(
        run_embed_pass(&pool, &embedder, &mut backoff, &cancel)
            .await
            .unwrap(),
        EmbedPassOutcome::Waiting
    );
    assert_eq!(fake.calls().len(), 1, "backing off");
    assert_eq!(chunk_embedding_counts(&admin, attachment).await.0, 0);
    let embedded_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.embedded'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(embedded_events, 0);

    // Hybrid with the provider failing: same lexical answer, lexical cursor.
    let (status, lexical) = search(
        &app,
        &owner_cookie,
        workspace_id,
        &token,
        "mode=lexical&limit=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, hybrid) = search(
        &app,
        &owner_cookie,
        workspace_id,
        &token,
        "mode=hybrid&limit=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hybrid}");
    assert_eq!(ids_of(&hybrid), ids_of(&lexical));
    let cursor = hybrid["nextCursor"].as_str().expect("cursor").to_string();
    let (status, next) = search(
        &app,
        &owner_cookie,
        workspace_id,
        &token,
        &format!("mode=lexical&limit=1&cursor={}", urlencoding(&cursor)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "fallback cursor is lexical: {next}");
    assert_eq!(ids_of(&next).len(), 1);

    // A malformed provider answer (wrong dimension) is refused, not stored.
    fake.set_mode(MODE_WRONG_DIM);
    let mut fresh = EmbedBackoff::default();
    assert_eq!(
        run_embed_pass(&pool, &embedder, &mut fresh, &cancel)
            .await
            .unwrap(),
        EmbedPassOutcome::Waiting
    );
    assert_eq!(chunk_embedding_counts(&admin, attachment).await.0, 0);

    // Unreachable provider: the search still answers lexically.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let down_app = search_app(
        &harness,
        &meili,
        Some(embedder_for(&format!("http://{closed}/v1"))),
    )
    .await;
    let (status, down) = search(
        &down_app,
        &owner_cookie,
        workspace_id,
        &token,
        "mode=hybrid&limit=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{down}");
    assert_eq!(ids_of(&down), ids_of(&lexical));

    // Provider back: a later pass converges and hybrid ranks semantically.
    fake.set_mode(MODE_OK);
    assert_eq!(embed_all(&pool, &embedder).await, vec![attachment]);
    deliver_events(&admin, &pool, &meili, &mut delivered).await;
    let (_, cat) = search(&app, &owner_cookie, workspace_id, "kitten", "mode=hybrid").await;
    assert_eq!(ids_of(&cat), vec![attachment.to_string()]);

    // Without an embedder configured, hybrid is plain lexical and never calls out.
    let plain = search_app(&harness, &meili, None).await;
    let calls = fake.calls().len();
    let (status, body) = search(&plain, &owner_cookie, workspace_id, "kitten", "mode=hybrid").await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids_of(&body).is_empty());
    assert_eq!(fake.calls().len(), calls);

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}
