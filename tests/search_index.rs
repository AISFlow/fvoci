#![cfg(feature = "db-tests")]

use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::Duration;

use fvoci_server::db::attachment_extract::{finish_extract, ExtractClaim, FinishExtract};
use fvoci_server::db::outbox::is_processed;
use fvoci_server::db::outbox::{fetch_cursor, fetch_failure_state, OutboxEvent};
use fvoci_server::db::{migrate, pool};
use fvoci_server::outbox::spawn_outbox_dispatcher;
use fvoci_server::outbox::OutboxDispatcherSettings;
use fvoci_server::search::chunk::chunk_plain_text;
use fvoci_server::search::index::{
    process_search_index_event, rebuild_pool, rebuild_search_index, search_index_consumer,
    SEARCH_INDEX_CONSUMER,
};
use fvoci_server::search::meili::{
    delete_all_meili_documents, ensure_meili_index, search_meili, search_source_id,
    wait_meili_tasks, MeiliConfig, MeiliError, MeiliSearchInput, MeiliSearchScope,
    SearchSourceKind,
};
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

/// One Meili CE + one Postgres accept these tests; running them in parallel
/// makes `meili_down_retries_*` miss its 15s dispatcher wait while other
/// cases create indexes and upsert. Kept after removing the process-wide Meili
/// write-batch static: shared CE/Postgres contention is unchanged.
static SEARCH_INDEX_EXCLUSIVE: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
    _exclusive: MutexGuard<'static, ()>,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let exclusive = SEARCH_INDEX_EXCLUSIVE.lock().await;
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");
        let db_name = format!("fvoci_sidx_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);
        let admin_pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;
        let admin_url = join_db_url(&server_url, &db_name);
        migrate::run_migrations(&admin_url).await.expect("migrate");
        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOBYPASSRLS",
            role_name, role_password
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
        migration_pool.close().await;
        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();
        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
            _exclusive: exclusive,
        }
    }

    async fn cleanup(self) {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .ok();
        if let Some(pool) = pool {
            let _ = sqlx::query(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ))
            .execute(&pool)
            .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
                .execute(&pool)
                .await;
            let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed.clone();
    server.set_path("/postgres");
    server.to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut url = url::Url::parse(server_url).expect("database url");
    url.set_path(&format!("/{db_name}"));
    url.to_string()
}

fn test_meili() -> MeiliConfig {
    let url = std::env::var("FVOCI_MEILI_URL").expect("FVOCI_MEILI_URL");
    let key = std::env::var("FVOCI_MEILI_KEY").expect("FVOCI_MEILI_KEY");
    MeiliConfig::new(url, key, format!("fvoci_{}", Uuid::now_v7().simple()))
}

fn path_label(id: Uuid) -> String {
    id.simple().to_string()
}

struct Fixture {
    owner_id: Uuid,
    workspace_id: Uuid,
    wiki_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    comment_id: Uuid,
    attachment_id: Uuid,
    token: String,
}

async fn seed(admin: &PgPool, token: &str) -> Fixture {
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    let wiki_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let task_id = Uuid::now_v7();
    let comment_id = Uuid::now_v7();
    let attachment_id = Uuid::now_v7();
    let status_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Idx')")
        .bind(user_id)
        .bind(format!("idx-{}@example.com", user_id.simple()))
        .execute(admin)
        .await
        .expect("user");
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'ws')")
        .bind(workspace_id)
        .bind(format!("s{}", &user_id.simple().to_string()[..16]))
        .execute(admin)
        .await
        .expect("workspace");
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(admin)
    .await
    .expect("membership");
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number,
            status, schema_version, text, chosung, created_by, content_json, kind
        ) VALUES (
            $1, $2, $3, $4, NULL, 'V', NULL, 1, 'published', 1, $3, '', $5,
            '{"type":"doc","content":[]}'::jsonb, 'wiki'
        )
        "#,
    )
    .bind(wiki_id)
    .bind(workspace_id)
    .bind(token)
    .bind(path_label(wiki_id))
    .bind(user_id)
    .execute(admin)
    .await
    .expect("wiki");
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (id, workspace_id, key, name, visibility, created_by)
        VALUES ($1, $2, 'AB', 'Board', 'private', $3)
        "#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(user_id)
    .execute(admin)
    .await
    .expect("project");
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES ($1,$2,$3,$4,'lead')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(user_id)
    .execute(admin)
    .await
    .expect("lead");
    sqlx::query("INSERT INTO fvoci.workflows (id, workspace_id, project_id) VALUES ($1,$2,$3)")
        .bind(workflow_id)
        .bind(workspace_id)
        .bind(project_id)
        .execute(admin)
        .await
        .expect("workflow");
    sqlx::query(
        r#"
        INSERT INTO fvoci.statuses (id, workspace_id, project_id, workflow_id, name, category, sort_key)
        VALUES ($1,$2,$3,$4,'Todo','todo','V')
        "#,
    )
    .bind(status_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(workflow_id)
    .execute(admin)
    .await
    .expect("status");
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, status_id, created_by, content_json
        ) VALUES ($1,$2,$3,1,$4,$5,$6,'{"type":"doc","content":[]}'::jsonb)
        "#,
    )
    .bind(task_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(token)
    .bind(status_id)
    .bind(user_id)
    .execute(admin)
    .await
    .expect("task");
    sqlx::query(
        r#"
        INSERT INTO fvoci.comments (id, workspace_id, document_id, created_by, body)
        VALUES ($1,$2,$3,$4,$5)
        "#,
    )
    .bind(comment_id)
    .bind(workspace_id)
    .bind(wiki_id)
    .bind(user_id)
    .bind(token)
    .execute(admin)
    .await
    .expect("comment");
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, extract_status, extract_text, completed_at
        ) VALUES (
            $1,$2,$3,$4,'stored',$6, 12, 12, $5, 'pending', '', now()
        )
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(wiki_id)
    .bind(user_id)
    .bind(format!("att/{attachment_id}"))
    .bind(token)
    .execute(admin)
    .await
    .expect("attachment");
    Fixture {
        owner_id: user_id,
        workspace_id,
        wiki_id,
        project_id,
        task_id,
        comment_id,
        attachment_id,
        token: token.to_string(),
    }
}

async fn insert_event(
    admin: &PgPool,
    workspace_id: Uuid,
    verb: &str,
    target_type: &str,
    target_id: Uuid,
) -> OutboxEvent {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, payload, channel)
        VALUES ($1,$2,$3,$4,$5,'{}'::jsonb,'system')
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(verb)
    .bind(target_type)
    .bind(target_id)
    .execute(admin)
    .await
    .expect("event");
    fvoci_server::db::outbox::fetch_event_by_id(admin, id)
        .await
        .expect("fetch")
        .expect("row")
}

async fn search_hits(
    meili: &MeiliConfig,
    workspace_id: Uuid,
    project_id: Uuid,
    token: &str,
) -> Vec<String> {
    let page = search_meili(
        meili,
        &MeiliSearchInput {
            q: token.to_string(),
            stem: String::new(),
            scopes: vec![MeiliSearchScope {
                workspace_id: workspace_id.to_string(),
                project_ids: vec![project_id.to_string()],
                include_wiki: true,
                wiki_document_ids: Vec::new(),
            }],
            kind: None,
            limit: 50,
            offset: 0,
        },
    )
    .await
    .expect("search");
    page.hits.into_iter().map(|h| h.id).collect()
}

async fn meili_ids(meili: &MeiliConfig) -> Vec<String> {
    let url = format!(
        "{}/indexes/{}/documents?limit=1000",
        meili.url, meili.index_uid
    );
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(meili.api_key())
        .send()
        .await
        .expect("list");
    assert_eq!(resp.status().as_u16(), 200, "list documents");
    let body: Value = resp.json().await.expect("json");
    let results = body
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut ids: Vec<String> = results
        .iter()
        .filter_map(|row| row.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

async fn meili_doc(meili: &MeiliConfig, id: &str) -> Value {
    let url = format!("{}/indexes/{}/documents/{}", meili.url, meili.index_uid, id);
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(meili.api_key())
        .send()
        .await
        .expect("get document");
    assert_eq!(resp.status().as_u16(), 200, "get document {id}");
    resp.json().await.expect("json")
}

async fn enqueue_raw_documents(meili: &MeiliConfig, docs: Value) -> u64 {
    let url = format!("{}/indexes/{}/documents", meili.url, meili.index_uid);
    let resp = reqwest::Client::new()
        .post(url)
        .bearer_auth(meili.api_key())
        .json(&docs)
        .send()
        .await
        .expect("enqueue raw");
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.expect("enqueue json");
    assert!(
        (200..300).contains(&status),
        "enqueue HTTP {status}: {body}"
    );
    body.get("taskUid")
        .and_then(Value::as_u64)
        .expect("taskUid")
}

fn raw_meili_doc(id: &str, title: &str) -> Value {
    json!({
        "id": id,
        "kind": "document",
        "title": title,
        "_vectors": { "attachments": null },
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_project_document(
    admin: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    owner_id: Uuid,
    doc_id: Uuid,
    parent_id: Option<Uuid>,
    path: &str,
    title: &str,
    number: i32,
) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number,
            status, schema_version, text, chosung, created_by, content_json, kind
        ) VALUES (
            $1, $2, $3, $4, $5, 'V', $6, $7, 'published', 1, $3, '', $8,
            '{"type":"doc","content":[]}'::jsonb, 'wiki'
        )
        "#,
    )
    .bind(doc_id)
    .bind(workspace_id)
    .bind(title)
    .bind(path)
    .bind(parent_id)
    .bind(project_id)
    .bind(number)
    .bind(owner_id)
    .execute(admin)
    .await
    .expect("project document");
}

async fn wait_until<F>(timeout: Duration, what: &str, mut predicate: F)
where
    F: FnMut() -> Pin<Box<dyn Future<Output = bool> + Send>>,
{
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if predicate().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{what}: condition not met within {timeout:?}");
}

#[tokio::test]
async fn indexes_each_resource_type_from_events() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxallkinds").await;

    for (verb, ty, id) in [
        ("document.created", "document", fixture.wiki_id),
        ("task.created", "task", fixture.task_id),
        ("comment.created", "comment", fixture.comment_id),
        ("attachment.created", "attachment", fixture.attachment_id),
    ] {
        let event = insert_event(&admin, fixture.workspace_id, verb, ty, id).await;
        process_search_index_event(&app, &meili, &event)
            .await
            .expect("index");
    }

    let hits = search_hits(
        &meili,
        fixture.workspace_id,
        fixture.project_id,
        &fixture.token,
    )
    .await;
    assert!(hits.contains(&search_source_id(
        SearchSourceKind::Document,
        &fixture.wiki_id.to_string(),
        None
    )));
    assert!(hits.contains(&search_source_id(
        SearchSourceKind::Task,
        &fixture.task_id.to_string(),
        None
    )));
    assert!(hits.contains(&search_source_id(
        SearchSourceKind::Comment,
        &fixture.comment_id.to_string(),
        None
    )));
    assert!(hits.contains(&search_source_id(
        SearchSourceKind::Attachment,
        &fixture.attachment_id.to_string(),
        None
    )));

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_to_private_project_move_updates_project_id() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxmove").await;
    let created = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &created)
        .await
        .expect("created");

    sqlx::query("UPDATE fvoci.documents SET project_id = $3 WHERE workspace_id = $1 AND id = $2")
        .bind(fixture.workspace_id)
        .bind(fixture.wiki_id)
        .bind(fixture.project_id)
        .execute(&admin)
        .await
        .expect("move row");
    let mut moved = insert_event(
        &admin,
        fixture.workspace_id,
        "document.moved",
        "document",
        fixture.wiki_id,
    )
    .await;
    moved.payload = json!({
        "oldProjectId": null,
        "newProjectId": fixture.project_id.to_string(),
    });
    process_search_index_event(&app, &meili, &moved)
        .await
        .expect("moved");

    let url = format!(
        "{}/indexes/{}/documents/{}",
        meili.url,
        meili.index_uid,
        search_source_id(
            SearchSourceKind::Document,
            &fixture.wiki_id.to_string(),
            None
        )
    );
    let doc: Value = reqwest::Client::new()
        .get(url)
        .bearer_auth(meili.api_key())
        .send()
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    assert_eq!(
        doc.get("projectId").and_then(|v| v.as_str()),
        Some(fixture.project_id.to_string().as_str())
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn trash_removes_and_restore_reindexes() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxtrash").await;
    let created = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &created)
        .await
        .expect("created");

    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("trash");
    let trashed = insert_event(
        &admin,
        fixture.workspace_id,
        "document.trashed",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &trashed)
        .await
        .expect("trashed");
    let hits = search_hits(
        &meili,
        fixture.workspace_id,
        fixture.project_id,
        &fixture.token,
    )
    .await;
    assert!(!hits.iter().any(|id| id.starts_with("document_")));

    sqlx::query("UPDATE fvoci.documents SET deleted_at = NULL WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("restore row");
    let restored = insert_event(
        &admin,
        fixture.workspace_id,
        "document.restored",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &restored)
        .await
        .expect("restored");
    let hits = search_hits(
        &meili,
        fixture.workspace_id,
        fixture.project_id,
        &fixture.token,
    )
    .await;
    assert!(hits.contains(&search_source_id(
        SearchSourceKind::Document,
        &fixture.wiki_id.to_string(),
        None
    )));

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn attachment_chunks_index_after_extract_and_drop_with_attachment() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxchunk").await;
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
    .bind(fixture.attachment_id)
    .bind(lease)
    .execute(&admin)
    .await
    .expect("lease");
    let long = format!("{}\n\n{}", "qvoxchunk ".repeat(200), "tail ".repeat(200));
    assert!(chunk_plain_text(&long).len() >= 2);
    let claim = ExtractClaim {
        workspace_id: fixture.workspace_id,
        attachment_id: fixture.attachment_id,
        lease_token: lease,
        attempt: 1,
    };
    let applied = finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "ok".into(),
            text: long.clone(),
            warnings: Vec::new(),
            rhwp_rev: None,
        },
    )
    .await
    .expect("finish");
    assert!(applied);
    let event = fvoci_server::db::outbox::fetch_event_by_id(&admin, {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM fvoci.events WHERE verb = 'attachment.extracted' AND target_id = $1",
        )
        .bind(fixture.attachment_id)
        .fetch_one(&admin)
        .await
        .expect("extracted event")
    })
    .await
    .expect("fetch")
    .expect("row");
    process_search_index_event(&app, &meili, &event)
        .await
        .expect("index chunks");
    let chunk_id = search_source_id(
        SearchSourceKind::Attachment,
        &fixture.attachment_id.to_string(),
        Some(0),
    );
    let ids = meili_ids(&meili).await;
    assert!(ids.contains(&chunk_id), "{ids:?}");

    sqlx::query("DELETE FROM fvoci.attachments WHERE id = $1")
        .bind(fixture.attachment_id)
        .execute(&admin)
        .await
        .expect("delete att");
    let deleted = insert_event(
        &admin,
        fixture.workspace_id,
        "attachment.deleted",
        "attachment",
        fixture.attachment_id,
    )
    .await;
    process_search_index_event(&app, &meili, &deleted)
        .await
        .expect("delete index");
    let ids = meili_ids(&meili).await;
    assert!(!ids
        .iter()
        .any(|id| id.contains(&fixture.attachment_id.to_string())));

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_delete_purges_index() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxpurge").await;
    let created = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &created)
        .await
        .expect("created");
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(fixture.workspace_id)
        .execute(&admin)
        .await
        .expect("ws delete");
    let deleted = insert_event(
        &admin,
        fixture.workspace_id,
        "workspace.deleted",
        "workspace",
        fixture.workspace_id,
    )
    .await;
    process_search_index_event(&app, &meili, &deleted)
        .await
        .expect("purged");
    assert!(meili_ids(&meili).await.is_empty());

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn duplicate_delivery_is_idempotent() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxidem").await;
    let event = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &event)
        .await
        .expect("first");
    process_search_index_event(&app, &meili, &event)
        .await
        .expect("second");
    let ids = meili_ids(&meili).await;
    assert_eq!(
        ids.iter().filter(|id| id.starts_with("document_")).count(),
        1
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn meili_down_retries_without_advancing_then_converges() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let good = test_meili();
    ensure_meili_index(&good).await.expect("ensure");
    let fixture = seed(&admin, "qvoxdown").await;
    let event = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;
    let down = MeiliConfig::new(
        "http://127.0.0.1:57999".into(),
        "test-key-at-least-16".into(),
        good.index_uid.clone(),
    );
    let consumer = search_index_consumer(down);
    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 10,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![consumer],
    )
    .expect("dispatcher");
    wait_until(
        Duration::from_secs(15),
        "meili-down failure recorded",
        || {
            let pool = app.clone();
            let id = event.id;
            Box::pin(async move {
                fetch_failure_state(&pool, SEARCH_INDEX_CONSUMER, id)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
            })
        },
    )
    .await;
    let cursor = fetch_cursor(&admin, SEARCH_INDEX_CONSUMER)
        .await
        .expect("cursor");
    assert!(
        cursor.is_none() || cursor == Some(("0".into(), 0)),
        "cursor must not advance while Meili is down: {cursor:?}"
    );
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join down");

    let ok = search_index_consumer(good.clone());
    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(2),
            batch_limit: 10,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![ok],
    )
    .expect("dispatcher up");
    wait_until(Duration::from_secs(15), "meili-up search hits", || {
        let meili = good.clone();
        let ws = fixture.workspace_id;
        let token = fixture.token.clone();
        let project = fixture.project_id;
        Box::pin(async move { !search_hits(&meili, ws, project, &token).await.is_empty() })
    })
    .await;
    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join up");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn rebuild_from_empty_index_converges() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxrebuild").await;
    for (verb, ty, id) in [
        ("document.created", "document", fixture.wiki_id),
        ("task.created", "task", fixture.task_id),
        ("comment.created", "comment", fixture.comment_id),
        ("attachment.created", "attachment", fixture.attachment_id),
    ] {
        let event = insert_event(&admin, fixture.workspace_id, verb, ty, id).await;
        process_search_index_event(&app, &meili, &event)
            .await
            .expect("index");
    }
    let before = meili_ids(&meili).await;
    assert!(!before.is_empty());
    delete_all_meili_documents(&meili).await.expect("clear");
    assert!(meili_ids(&meili).await.is_empty());
    // The CLI's own pool, so the rebuild's concurrent connection need is exercised.
    let rebuild = rebuild_pool(&harness.admin_url)
        .await
        .expect("rebuild pool");
    rebuild_search_index(&rebuild, &meili, None)
        .await
        .expect("rebuild");
    rebuild.close().await;
    let after = meili_ids(&meili).await;
    assert_eq!(before, after);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn partial_extract_chunks_are_indexed() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxpartial").await;
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
    .bind(fixture.attachment_id)
    .bind(lease)
    .execute(&admin)
    .await
    .expect("lease");
    let long = format!("{}\n\n{}", "qvoxpartial ".repeat(200), "tail ".repeat(200));
    assert!(chunk_plain_text(&long).len() >= 2);
    let claim = ExtractClaim {
        workspace_id: fixture.workspace_id,
        attachment_id: fixture.attachment_id,
        lease_token: lease,
        attempt: 1,
    };
    let applied = finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "partial".into(),
            text: long.clone(),
            warnings: vec!["truncated".into()],
            rhwp_rev: None,
        },
    )
    .await
    .expect("finish");
    assert!(applied);
    let chunks: Vec<(i32, String)> = sqlx::query_as(
        "SELECT chunk_no, status FROM fvoci.attachment_text WHERE attachment_id = $1 ORDER BY chunk_no",
    )
    .bind(fixture.attachment_id)
    .fetch_all(&admin)
    .await
    .expect("chunks");
    assert!(chunks.len() >= 2, "{chunks:?}");
    assert!(chunks.iter().all(|(_, status)| status == "partial"));

    let event = fvoci_server::db::outbox::fetch_event_by_id(&admin, {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM fvoci.events WHERE verb = 'attachment.extracted' AND target_id = $1",
        )
        .bind(fixture.attachment_id)
        .fetch_one(&admin)
        .await
        .expect("extracted event")
    })
    .await
    .expect("fetch")
    .expect("row");
    process_search_index_event(&app, &meili, &event)
        .await
        .expect("index chunks");
    let chunk_id = search_source_id(
        SearchSourceKind::Attachment,
        &fixture.attachment_id.to_string(),
        Some(0),
    );
    let ids = meili_ids(&meili).await;
    assert!(ids.contains(&chunk_id), "{ids:?}");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_body_refresh_skips_comments_and_chunks_until_title_or_trash() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxn4body").await;
    for (verb, ty, id) in [
        ("document.created", "document", fixture.wiki_id),
        ("comment.created", "comment", fixture.comment_id),
    ] {
        let event = insert_event(&admin, fixture.workspace_id, verb, ty, id).await;
        process_search_index_event(&app, &meili, &event)
            .await
            .expect("index");
    }

    let comment_meili_id = search_source_id(
        SearchSourceKind::Comment,
        &fixture.comment_id.to_string(),
        None,
    );
    let before = meili_doc(&meili, &comment_meili_id).await;
    assert_eq!(before["body"].as_str().unwrap(), "qvoxn4body");

    sqlx::query("UPDATE fvoci.comments SET body = 'stale-should-not-index' WHERE id = $1")
        .bind(fixture.comment_id)
        .execute(&admin)
        .await
        .expect("mutate comment off-event");
    sqlx::query("UPDATE fvoci.documents SET text = 'collab body now' WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("mutate document body");

    let mut collab = insert_event(
        &admin,
        fixture.workspace_id,
        "document.collab_update_appended",
        "document",
        fixture.wiki_id,
    )
    .await;
    collab.payload = json!({"documentId": fixture.wiki_id.to_string(), "seq": 1});
    process_search_index_event(&app, &meili, &collab)
        .await
        .expect("collab");

    let after_collab = meili_doc(&meili, &comment_meili_id).await;
    assert_eq!(
        after_collab["body"].as_str().unwrap(),
        "qvoxn4body",
        "collab refresh reindexed comments: {after_collab:?}"
    );
    let doc = meili_doc(
        &meili,
        &search_source_id(
            SearchSourceKind::Document,
            &fixture.wiki_id.to_string(),
            None,
        ),
    )
    .await;
    assert!(
        doc["body"].as_str().unwrap().contains("collab body now"),
        "{doc:?}"
    );

    sqlx::query("UPDATE fvoci.documents SET title = 'qvoxn4title' WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("title");
    let mut titled = insert_event(
        &admin,
        fixture.workspace_id,
        "document.updated",
        "document",
        fixture.wiki_id,
    )
    .await;
    titled.payload = json!({
        "documentId": fixture.wiki_id.to_string(),
        "title": "qvoxn4title",
    });
    process_search_index_event(&app, &meili, &titled)
        .await
        .expect("title");
    let after_title = meili_doc(&meili, &comment_meili_id).await;
    assert_eq!(after_title["title"].as_str().unwrap(), "qvoxn4title");
    assert_eq!(
        after_title["body"].as_str().unwrap(),
        "stale-should-not-index"
    );

    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("trash");
    let trashed = insert_event(
        &admin,
        fixture.workspace_id,
        "document.trashed",
        "document",
        fixture.wiki_id,
    )
    .await;
    process_search_index_event(&app, &meili, &trashed)
        .await
        .expect("trashed");
    let ids = meili_ids(&meili).await;
    assert!(
        !ids.contains(&comment_meili_id),
        "trashed parent left comment indexed: {ids:?}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn burst_of_events_converges_with_one_meili_wait() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxburst").await;

    let n = 8usize;
    let mut events = Vec::new();
    for i in 0..n {
        let doc_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.documents (
                id, workspace_id, title, path, parent_id, sort_key, project_id, number,
                status, schema_version, text, chosung, created_by, content_json, kind
            ) VALUES (
                $1, $2, $3, $4, NULL, 'V', NULL, $5, 'published', 1, $3, '', $6,
                '{"type":"doc","content":[]}'::jsonb, 'wiki'
            )
            "#,
        )
        .bind(doc_id)
        .bind(fixture.workspace_id)
        .bind(format!("burst-{i}"))
        .bind(path_label(doc_id))
        .bind((i + 2) as i32)
        .bind(fixture.owner_id)
        .execute(&admin)
        .await
        .expect("doc");
        events.push(
            insert_event(
                &admin,
                fixture.workspace_id,
                "document.created",
                "document",
                doc_id,
            )
            .await,
        );
    }

    let consumer = search_index_consumer(meili.clone());
    let started = std::time::Instant::now();
    let (done, err) = consumer.deliver_batch(&app, Uuid::now_v7(), &events).await;
    let elapsed = started.elapsed();
    assert_eq!(done, n);
    assert!(err.is_none(), "batch delivery failed: {:?}", err);
    assert!(
        elapsed < Duration::from_secs(6),
        "burst took {:?}; expected one Meili wait chain",
        elapsed
    );

    let hits = search_hits(&meili, fixture.workspace_id, fixture.project_id, "burst-").await;
    assert_eq!(hits.len(), n);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn failed_middle_batch_event_never_marks_processed_and_retries_converge() {
    let harness = TestDb::bootstrap().await;
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");

    let uid_ok_a = enqueue_raw_documents(&meili, json!([raw_meili_doc("doc_ok_a", "ok-a")])).await;
    let uid_bad =
        enqueue_raw_documents(&meili, json!([raw_meili_doc("bad id!", "middle-fail")])).await;
    let uid_ok_c = enqueue_raw_documents(&meili, json!([raw_meili_doc("doc_ok_c", "ok-c")])).await;
    assert_ne!(uid_ok_a, 0);
    assert_ne!(uid_bad, 0);
    assert_ne!(uid_ok_c, 0);

    let err = wait_meili_tasks(&meili, &[uid_ok_a, uid_bad, uid_ok_c])
        .await
        .expect_err("middle uid must fail the wait even if the last task succeeds");
    assert_eq!(
        err,
        MeiliError::TaskFailed,
        "async Meili rejection, not a pre-enqueue DocumentTooLarge: {err}"
    );

    wait_meili_tasks(&meili, &[uid_ok_a, uid_ok_c])
        .await
        .expect("good prefix and suffix tasks still succeed on retry");
    let ids = meili_ids(&meili).await;
    assert!(ids.contains(&"doc_ok_a".to_string()), "{ids:?}");
    assert!(ids.contains(&"doc_ok_c".to_string()), "{ids:?}");
    assert!(!ids.iter().any(|id| id.contains(' ')), "{ids:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn wait_meili_tasks_verifies_more_than_twenty_uids() {
    let harness = TestDb::bootstrap().await;
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");

    let mut uids = Vec::new();
    for i in 0..25 {
        let uid = enqueue_raw_documents(
            &meili,
            json!([raw_meili_doc(
                &format!("doc_page_{i}"),
                &format!("page-{i}")
            )]),
        )
        .await;
        uids.push(uid);
    }
    wait_meili_tasks(&meili, &uids)
        .await
        .expect("waiting on 25 uids must page past Meili's default limit of 20");
    let ids = meili_ids(&meili).await;
    assert_eq!(ids.len(), 25, "{ids:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn parallel_dispatchers_do_not_cross_talk_in_one_process() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app_a = pool::connect_app(&harness.app_url).await.expect("app a");
    let app_b = pool::connect_app(&harness.app_url).await.expect("app b");
    let meili_a = test_meili();
    let meili_b = test_meili();
    let meili_fail = test_meili();
    ensure_meili_index(&meili_a).await.expect("ensure a");
    ensure_meili_index(&meili_b).await.expect("ensure b");
    ensure_meili_index(&meili_fail).await.expect("ensure fail");
    let fixture_a = seed(&admin, "qvoxpara").await;
    let fixture_b = seed(&admin, "qvoxparb").await;

    let mut batch_a = Vec::new();
    let mut batch_b = Vec::new();
    for i in 0..3 {
        let doc_a = Uuid::now_v7();
        let doc_b = Uuid::now_v7();
        insert_project_document(
            &admin,
            fixture_a.workspace_id,
            fixture_a.project_id,
            fixture_a.owner_id,
            doc_a,
            None,
            &path_label(doc_a),
            &format!("qvoxpara-{i}"),
            i + 20,
        )
        .await;
        insert_project_document(
            &admin,
            fixture_b.workspace_id,
            fixture_b.project_id,
            fixture_b.owner_id,
            doc_b,
            None,
            &path_label(doc_b),
            &format!("qvoxparb-{i}"),
            i + 20,
        )
        .await;
        batch_a.push(
            insert_event(
                &admin,
                fixture_a.workspace_id,
                "document.created",
                "document",
                doc_a,
            )
            .await,
        );
        batch_b.push(
            insert_event(
                &admin,
                fixture_b.workspace_id,
                "document.created",
                "document",
                doc_b,
            )
            .await,
        );
    }

    let uid_ok =
        enqueue_raw_documents(&meili_fail, json!([raw_meili_doc("parallel_ok", "ok")])).await;
    let uid_bad =
        enqueue_raw_documents(&meili_fail, json!([raw_meili_doc("bad id!", "fail")])).await;
    let uid_ok2 =
        enqueue_raw_documents(&meili_fail, json!([raw_meili_doc("parallel_ok2", "ok2")])).await;

    let consumer_a = search_index_consumer(meili_a.clone());
    let consumer_b = search_index_consumer(meili_b.clone());
    let fail_uids = [uid_ok, uid_bad, uid_ok2];
    let (res_a, res_b, wait_fail) = tokio::join!(
        consumer_a.deliver_batch(&app_a, Uuid::now_v7(), &batch_a),
        consumer_b.deliver_batch(&app_b, Uuid::now_v7(), &batch_b),
        wait_meili_tasks(&meili_fail, &fail_uids),
    );
    assert_eq!(res_a.0, 3);
    assert!(res_a.1.is_none(), "A failed: {:?}", res_a.1);
    assert_eq!(res_b.0, 3);
    assert!(res_b.1.is_none(), "B failed: {:?}", res_b.1);
    assert_eq!(
        wait_fail.expect_err("shared last-uid wait would hide the middle failure"),
        MeiliError::TaskFailed
    );

    let hits_a = search_hits(
        &meili_a,
        fixture_a.workspace_id,
        fixture_a.project_id,
        "qvoxpara",
    )
    .await;
    let hits_b = search_hits(
        &meili_b,
        fixture_b.workspace_id,
        fixture_b.project_id,
        "qvoxparb",
    )
    .await;
    assert_eq!(hits_a.len(), 3, "A must contain its own batch: {hits_a:?}");
    assert_eq!(hits_b.len(), 3, "B must contain its own batch: {hits_b:?}");
    assert!(
        search_hits(
            &meili_a,
            fixture_b.workspace_id,
            fixture_b.project_id,
            "qvoxparb"
        )
        .await
        .is_empty(),
        "index A must not contain B's documents"
    );
    assert!(
        search_hits(
            &meili_b,
            fixture_a.workspace_id,
            fixture_a.project_id,
            "qvoxpara"
        )
        .await
        .is_empty(),
        "index B must not contain A's documents"
    );

    app_a.close().await;
    app_b.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn batch_coalesce_keeps_subtree_after_move_then_update() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxcoal").await;

    let parent_id = Uuid::now_v7();
    let child_id = Uuid::now_v7();
    let new_project = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (id, workspace_id, key, name, visibility, created_by)
        VALUES ($1, $2, 'CD', 'Moved', 'private', $3)
        "#,
    )
    .bind(new_project)
    .bind(fixture.workspace_id)
    .bind(fixture.owner_id)
    .execute(&admin)
    .await
    .expect("new project");

    let parent_path = path_label(parent_id);
    insert_project_document(
        &admin,
        fixture.workspace_id,
        fixture.project_id,
        fixture.owner_id,
        parent_id,
        None,
        &parent_path,
        "coal-parent",
        20,
    )
    .await;
    insert_project_document(
        &admin,
        fixture.workspace_id,
        fixture.project_id,
        fixture.owner_id,
        child_id,
        Some(parent_id),
        &format!("{parent_path}.{}", path_label(child_id)),
        "coal-child",
        21,
    )
    .await;

    let consumer = search_index_consumer(meili.clone());
    let created = [
        insert_event(
            &admin,
            fixture.workspace_id,
            "document.created",
            "document",
            parent_id,
        )
        .await,
        insert_event(
            &admin,
            fixture.workspace_id,
            "document.created",
            "document",
            child_id,
        )
        .await,
    ];
    let (done, err) = consumer.deliver_batch(&app, Uuid::now_v7(), &created).await;
    assert_eq!(done, 2);
    assert!(err.is_none(), "{err:?}");

    sqlx::query("UPDATE fvoci.documents SET project_id = $1 WHERE id = ANY($2)")
        .bind(new_project)
        .bind([parent_id, child_id])
        .execute(&admin)
        .await
        .expect("move subtree in pg");

    let mut moved = insert_event(
        &admin,
        fixture.workspace_id,
        "document.moved",
        "document",
        parent_id,
    )
    .await;
    moved.payload = json!({
        "oldProjectId": fixture.project_id.to_string(),
        "newProjectId": new_project.to_string(),
    });
    let mut updated = insert_event(
        &admin,
        fixture.workspace_id,
        "document.updated",
        "document",
        parent_id,
    )
    .await;
    updated.payload = json!({ "collab": true });
    let (done, err) = consumer
        .deliver_batch(&app, Uuid::now_v7(), &[moved, updated])
        .await;
    assert_eq!(done, 2);
    assert!(err.is_none(), "{err:?}");

    let child_doc = meili_doc(
        &meili,
        &search_source_id(SearchSourceKind::Document, &child_id.to_string(), None),
    )
    .await;
    assert_eq!(
        child_doc.get("projectId").and_then(Value::as_str),
        Some(new_project.to_string().as_str()),
        "subtree refresh must follow a later body-only update: {child_doc}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn batch_coalesce_title_then_collab_refreshes_comments() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxtitle").await;

    let consumer = search_index_consumer(meili.clone());
    let created = [
        insert_event(
            &admin,
            fixture.workspace_id,
            "document.created",
            "document",
            fixture.wiki_id,
        )
        .await,
        insert_event(
            &admin,
            fixture.workspace_id,
            "comment.created",
            "comment",
            fixture.comment_id,
        )
        .await,
    ];
    let (done, err) = consumer.deliver_batch(&app, Uuid::now_v7(), &created).await;
    assert_eq!(done, 2);
    assert!(err.is_none());

    sqlx::query("UPDATE fvoci.documents SET title = 'qvoxtitle-new' WHERE id = $1")
        .bind(fixture.wiki_id)
        .execute(&admin)
        .await
        .expect("rename");

    let mut titled = insert_event(
        &admin,
        fixture.workspace_id,
        "document.updated",
        "document",
        fixture.wiki_id,
    )
    .await;
    titled.payload = json!({ "title": "qvoxtitle-new" });
    let mut collab = insert_event(
        &admin,
        fixture.workspace_id,
        "document.collab_update_appended",
        "document",
        fixture.wiki_id,
    )
    .await;
    collab.payload = json!({ "seq": 1 });
    let (done, err) = consumer
        .deliver_batch(&app, Uuid::now_v7(), &[titled, collab])
        .await;
    assert_eq!(done, 2);
    assert!(err.is_none(), "{err:?}");

    let comment = meili_doc(
        &meili,
        &search_source_id(
            SearchSourceKind::Comment,
            &fixture.comment_id.to_string(),
            None,
        ),
    )
    .await;
    assert_eq!(
        comment.get("title").and_then(Value::as_str),
        Some("qvoxtitle-new"),
        "title change must not be demoted to body-only by a later collab event: {comment}"
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn deliver_batch_respects_lease_equal_to_meili_timeout() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    ensure_meili_index(&meili).await.expect("ensure");
    let fixture = seed(&admin, "qvoxlease").await;

    let mut events = Vec::new();
    for i in 0..3 {
        let doc_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.documents (
                id, workspace_id, title, path, parent_id, sort_key, project_id, number,
                status, schema_version, text, chosung, created_by, content_json, kind
            ) VALUES (
                $1, $2, $3, $4, NULL, 'V', NULL, $5, 'published', 1, $3, '', $6,
                '{"type":"doc","content":[]}'::jsonb, 'wiki'
            )
            "#,
        )
        .bind(doc_id)
        .bind(fixture.workspace_id)
        .bind(format!("lease-{i}"))
        .bind(path_label(doc_id))
        .bind(i + 2)
        .bind(fixture.owner_id)
        .execute(&admin)
        .await
        .expect("doc");
        events.push(
            insert_event(
                &admin,
                fixture.workspace_id,
                "document.created",
                "document",
                doc_id,
            )
            .await,
        );
    }

    let consumer = search_index_consumer(meili.clone());
    let dispatcher = spawn_outbox_dispatcher(
        OutboxDispatcherSettings {
            poll_interval: Duration::from_millis(20),
            lease_ttl: Duration::from_secs(30),
            batch_limit: 10,
            failure_backoff: Duration::from_millis(50),
        },
        app.clone(),
        vec![consumer],
    )
    .expect("dispatcher");

    wait_until(
        Duration::from_secs(20),
        "lease-bound batch delivered",
        || {
            let pool = app.clone();
            let ids = events.iter().map(|e| e.id).collect::<Vec<_>>();
            Box::pin(async move {
                for id in ids {
                    if !is_processed(&pool, SEARCH_INDEX_CONSUMER, id)
                        .await
                        .unwrap_or(false)
                    {
                        return false;
                    }
                }
                true
            })
        },
    )
    .await;

    dispatcher.request_shutdown();
    dispatcher.join().await.expect("join");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn throughput_probe() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .expect("admin");
    let app = pool::connect_app(&harness.app_url).await.expect("app");
    let meili = test_meili();
    let t0 = std::time::Instant::now();
    ensure_meili_index(&meili).await.expect("ensure");
    let ensure_ms = t0.elapsed().as_millis();
    let fixture = seed(&admin, "qvoxprobe").await;
    let event = insert_event(
        &admin,
        fixture.workspace_id,
        "document.created",
        "document",
        fixture.wiki_id,
    )
    .await;

    let t1 = std::time::Instant::now();
    process_search_index_event(&app, &meili, &event)
        .await
        .expect("single");
    let single_ms = t1.elapsed().as_millis();

    let mut batch_events = Vec::new();
    for i in 0..5 {
        let doc_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.documents (
                id, workspace_id, title, path, parent_id, sort_key, project_id, number,
                status, schema_version, text, chosung, created_by, content_json, kind
            ) VALUES (
                $1, $2, $3, $4, NULL, 'V', NULL, $5, 'published', 1, $3, '', $6,
                '{"type":"doc","content":[]}'::jsonb, 'wiki'
            )
            "#,
        )
        .bind(doc_id)
        .bind(fixture.workspace_id)
        .bind(format!("probe-{i}"))
        .bind(path_label(doc_id))
        .bind(i + 10)
        .bind(fixture.owner_id)
        .execute(&admin)
        .await
        .expect("doc");
        batch_events.push(
            insert_event(
                &admin,
                fixture.workspace_id,
                "document.created",
                "document",
                doc_id,
            )
            .await,
        );
    }
    let consumer = search_index_consumer(meili.clone());
    let t2 = std::time::Instant::now();
    let (done, err) = consumer
        .deliver_batch(&app, Uuid::now_v7(), &batch_events)
        .await;
    assert_eq!(done, batch_events.len());
    assert!(err.is_none());
    let batch_ms = t2.elapsed().as_millis();

    println!(
        "THROUGHPUT_PROBE ensure_ms={} single_event_ms={} batch5_ms={}",
        ensure_ms, single_ms, batch_ms
    );

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}
