#![cfg(feature = "db-tests")]

#[path = "support/office_fixtures.rs"]
mod office_fixtures;

use fvoci_server::db::attachment_extract::{
    claim_extract, fetch_extract_state, finish_extract, load_extract_input, release_extract,
    FinishExtract, EXTRACT_MAX_ATTEMPTS,
};
use fvoci_server::db::{migrate, pool};
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");

        let db_name = format!("fvoci_ext_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);

        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
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
        apply_grants(&migration_pool, &role_name).await;
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();

        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
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
    let mut server = parsed;
    server.set_path("");
    server.to_string().trim_end_matches('/').to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{}", db_name));
    parsed.to_string()
}

async fn apply_grants(pool: &PgPool, role_name: &str) {
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
}

async fn app_pool(url: &str) -> PgPool {
    pool::connect_app(url).await.expect("connect app")
}

async fn seed_workspace(admin: &PgPool) -> (Uuid, Uuid, Uuid) {
    let workspace_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let document_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.users (id, email, password_hash, given_name, family_name, is_instance_admin)
        VALUES ($1, $2, 'hash', 'Test', 'User', true)
        "#,
    )
    .bind(user_id)
    .bind(format!("user-{}@example.test", user_id.simple()))
    .execute(admin)
    .await
    .expect("insert user");
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(workspace_id.simple().to_string().to_lowercase())
        .bind("Workspace")
        .execute(admin)
        .await
        .expect("insert workspace");
    sqlx::query(
        r#"
        INSERT INTO fvoci.memberships (workspace_id, user_id, role)
        VALUES ($1, $2, 'owner')
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(admin)
    .await
    .expect("insert membership");
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Doc', $3, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(document_id.simple().to_string())
    .bind(user_id)
    .execute(admin)
    .await
    .expect("insert document");
    (workspace_id, user_id, document_id)
}

async fn insert_pending_attachment(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    uploader_id: Uuid,
    name: &str,
) -> Uuid {
    let attachment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, mime,
            size_bytes, reserved_size_bytes, storage_key, image, scan_status,
            extract_status, completed_at
        )
        VALUES (
            $1, $2, $3, $4, 'stored', $5, 'application/x-hwp',
            16, 16, $6, false, 'skipped', 'pending', now()
        )
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(uploader_id)
    .bind(name)
    .bind(format!("attachments/{}/{}", workspace_id, attachment_id))
    .execute(admin)
    .await
    .expect("insert attachment");
    attachment_id
}

#[tokio::test]
async fn migration_007_columns_and_claim_function_exist() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations WHERE version = 7")
            .fetch_optional(&admin)
            .await
            .unwrap();
    assert_eq!(version, Some(7));
    let columns: (bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'fvoci' AND table_name = 'attachments'
                  AND column_name = 'extract_lease_token'
            ),
            EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'fvoci' AND table_name = 'attachments'
                  AND column_name = 'extract_attempts'
            ),
            EXISTS (
                SELECT 1 FROM pg_proc p
                JOIN pg_namespace n ON n.oid = p.pronamespace
                WHERE n.nspname = 'fvoci' AND p.proname = 'app_claim_attachment_extract'
            )
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(columns.0 && columns.1 && columns.2);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_claims_are_disjoint() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let a1 = insert_pending_attachment(&admin, workspace_id, document_id, user_id, "a.hwp").await;
    let a2 = insert_pending_attachment(&admin, workspace_id, document_id, user_id, "b.hwp").await;

    let app_b = app_pool(&harness.app_url).await;
    let (c1, c2) = tokio::join!(claim_extract(&app), claim_extract(&app_b));
    let c1 = c1.unwrap().expect("first claim");
    let c2 = c2.unwrap().expect("second claim");
    assert_ne!(c1.attachment_id, c2.attachment_id);
    assert!(
        (c1.attachment_id == a1 && c2.attachment_id == a2)
            || (c1.attachment_id == a2 && c2.attachment_id == a1)
    );

    app.close().await;
    app_b.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn public_role_cannot_execute_claim_function() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let denied_role = format!("fvoci_denied_{}", Uuid::now_v7().simple());
    sqlx::query(&format!(
        "CREATE ROLE \"{}\" LOGIN PASSWORD 'denied' NOSUPERUSER NOBYPASSRLS",
        denied_role
    ))
    .execute(&admin)
    .await
    .unwrap();
    let mut denied_url = url::Url::parse(&harness.admin_url).unwrap();
    denied_url.set_username(&denied_role).ok();
    denied_url.set_password(Some("denied")).ok();
    let denied_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(denied_url.as_str())
        .await
        .unwrap();
    let err = sqlx::query("SELECT * FROM fvoci.app_claim_attachment_extract()")
        .execute(&denied_pool)
        .await
        .expect_err("ungranted role must not execute claim");
    assert!(err.to_string().contains("permission denied"));
    denied_pool.close().await;
    let _ = sqlx::query(&format!("DROP ROLE \"{}\"", denied_role))
        .execute(&admin)
        .await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn partial_extract_writes_attachment_text_chunks() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "partial.hwp").await;
    let claim = claim_extract(&app).await.unwrap().expect("claim");
    finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "partial".into(),
            text: "partial extract body".into(),
            warnings: vec!["truncated".into()],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();

    let rows: Vec<(i32, String, String)> = sqlx::query_as(
        r#"
        SELECT chunk_no, status, text
        FROM fvoci.attachment_text
        WHERE workspace_id = $1 AND attachment_id = $2
        ORDER BY chunk_no
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, "partial");
    assert_eq!(rows[0].2, "partial extract body");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn system_ctx_cannot_read_other_tenant_extract_text() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (ws_a, user_a, doc_a) = seed_workspace(&admin).await;
    let (ws_b, user_b, doc_b) = seed_workspace(&admin).await;
    let att_a = insert_pending_attachment(&admin, ws_a, doc_a, user_a, "a.hwp").await;
    let att_b = insert_pending_attachment(&admin, ws_b, doc_b, user_b, "b.hwp").await;

    let claim_a = claim_extract(&app).await.unwrap().expect("claim a");
    assert_eq!(claim_a.attachment_id, att_a);
    finish_extract(
        &app,
        &claim_a,
        &FinishExtract {
            status: "ok".into(),
            text: "tenant-a-secret".into(),
            warnings: vec![],
            rhwp_rev: Some("rev".into()),
        },
    )
    .await
    .unwrap();

    let mut tx = app.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_b.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM fvoci.attachments
        WHERE workspace_id = $1 AND id = $2 AND extract_text = 'tenant-a-secret'
        "#,
    )
    .bind(ws_a)
    .bind(att_a)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count.0, 0);

    let state_b = fetch_extract_state(&app, ws_b, att_b)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state_b.extract_text, "");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn stale_lease_finish_matches_zero_rows() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "stale.hwp").await;

    let first = claim_extract(&app).await.unwrap().expect("first claim");
    assert_eq!(first.attachment_id, attachment_id);

    sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_lease_expires_at = now() - interval '1 second'
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .execute(&admin)
    .await
    .unwrap();

    let second = claim_extract(&app).await.unwrap().expect("reclaim");
    assert_ne!(second.lease_token, first.lease_token);

    let applied = finish_extract(
        &app,
        &first,
        &FinishExtract {
            status: "ok".into(),
            text: "stale-write".into(),
            warnings: vec![],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();
    assert!(!applied);

    finish_extract(
        &app,
        &second,
        &FinishExtract {
            status: "ok".into(),
            text: "fresh-write".into(),
            warnings: vec![],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_text, "fresh-write");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn parent_document_deletion_blocks_finish() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "orphan.hwp").await;
    let claim = claim_extract(&app).await.unwrap().expect("claim");

    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&admin)
    .await
    .unwrap();

    let applied = finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "ok".into(),
            text: "must-not-persist".into(),
            warnings: vec![],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();
    assert!(!applied);

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_text, "");
    assert_eq!(state.extract_status, "pending");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn retry_exhaustion_marks_worker_failure() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "fail.hwp").await;

    for _ in 0..EXTRACT_MAX_ATTEMPTS {
        let claim = claim_extract(&app).await.unwrap().expect("claim");
        sqlx::query(
            r#"
            UPDATE fvoci.attachments
            SET extract_lease_expires_at = now() - interval '1 second'
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&admin)
        .await
        .unwrap();
        let _ = claim;
    }

    sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET extract_lease_expires_at = now() - interval '1 second'
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .execute(&admin)
    .await
    .unwrap();

    let _ = claim_extract(&app).await.unwrap();

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_status, "worker_failure");
    assert_eq!(state.extract_text, "");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn graceful_release_does_not_burn_attempt() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "cancel.hwp").await;

    let claim = claim_extract(&app).await.unwrap().expect("claim");
    assert_eq!(claim.attempt, 1);
    release_extract(&app, &claim).await.unwrap();

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_attempts, 0);
    assert!(state.lease_token.is_none());

    let again = claim_extract(&app).await.unwrap().expect("reclaim");
    assert_eq!(again.attempt, 1);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn oversize_input_finishes_resource_limit_without_reading_bytes() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, mime,
            size_bytes, reserved_size_bytes, storage_key, image, scan_status,
            extract_status, completed_at
        )
        VALUES (
            $1, $2, $3, $4, 'stored', 'big.hwp', 'application/x-hwp',
            25000000, 25000000, $5, false, 'skipped', 'pending', now()
        )
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(user_id)
    .bind(format!("attachments/{}/{}", workspace_id, attachment_id))
    .execute(&admin)
    .await
    .unwrap();

    let claim = claim_extract(&app).await.unwrap().expect("claim");
    let input = load_extract_input(&app, &claim).await.unwrap().unwrap();
    assert_eq!(input.size_bytes, 25_000_000);
    finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "resource_limit".into(),
            text: String::new(),
            warnings: vec!["oversize".into()],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_status, "resource_limit");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn finish_first_then_document_delete_keeps_extract_text() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "live.hwp").await;
    let claim = claim_extract(&app).await.unwrap().expect("claim");
    finish_extract(
        &app,
        &claim,
        &FinishExtract {
            status: "ok".into(),
            text: "안녕".into(),
            warnings: vec![],
            rhwp_rev: None,
        },
    )
    .await
    .unwrap();

    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&admin)
    .await
    .unwrap();

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_text, "안녕");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn deletion_first_barrier_blocks_concurrent_finish() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let attachment_id =
        insert_pending_attachment(&admin, workspace_id, document_id, user_id, "race.hwp").await;
    let claim = claim_extract(&app).await.unwrap().expect("claim");

    let mut blocker = admin.begin().await.unwrap();
    sqlx::query("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
        .bind(workspace_id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query(
        "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();

    let finish_app = app_pool(&harness.app_url).await;
    let finish_claim = claim;
    let finish_handle = tokio::spawn(async move {
        finish_extract(
            &finish_app,
            &finish_claim,
            &FinishExtract {
                status: "ok".into(),
                text: "must-not-persist".into(),
                warnings: vec![],
                rhwp_rev: None,
            },
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *blocker)
    .await
    .unwrap();
    blocker.commit().await.unwrap();

    let applied = finish_handle.await.unwrap().unwrap();
    assert!(!applied);

    let state = fetch_extract_state(&app, workspace_id, attachment_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.extract_text, "");
    assert_eq!(state.extract_status, "pending");

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_006_upgrades_to_007_attachment_extract() {
    let admin_base = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing");
    let db_name = format!("fvoci_ext_upg_{}", Uuid::now_v7().simple());
    let server_url = server_db_url(&admin_base);
    let admin_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&server_url)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;

    let admin_url = join_db_url(&server_url, &db_name);
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/001_schema.sql"),
        include_str!("../migrations/002_functions.sql"),
        include_str!("../migrations/003_workspace.sql"),
        include_str!("../migrations/004_documents.sql"),
        include_str!("../migrations/005_collab_updates.sql"),
        include_str!("../migrations/006_attachments.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&migration_pool).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO fvoci.schema_migrations (version) VALUES (1), (2), (3), (4), (5), (6)",
    )
    .execute(&migration_pool)
    .await
    .unwrap();
    let has_lease: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'fvoci' AND table_name = 'attachments' AND column_name = 'extract_lease_token')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(!has_lease.0);
    migration_pool.close().await;

    migrate::run_migrations(&admin_url).await.unwrap();
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&migration_pool)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        fvoci_server::db::migrate::compiled_migration_count() as i64
    );
    let has_lease: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'fvoci' AND table_name = 'attachments' AND column_name = 'extract_lease_token')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_lease.0);
    migration_pool.close().await;

    let server_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .unwrap();
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&server_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db_name}\""))
        .execute(&server_pool)
        .await;
    server_pool.close().await;
}

/// Office / text attachments go through the `--internal-office-extract`
/// child (source officeparser extract-text); HWP without the native helper
/// is skipped instead of retried forever.
#[tokio::test]
async fn office_and_text_attachments_extract_through_the_office_child() {
    use fvoci_server::attachments::{spawn_extract_job, ExtractJobSettings, ObjectStorage};
    use fvoci_server::documents::office::OfficeLimits;
    use std::time::Duration;

    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness.app_url).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (workspace_id, user_id, document_id) = seed_workspace(&admin).await;
    let root = std::env::temp_dir().join(format!("fvoci-office-extract-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let storage = ObjectStorage::local(root.clone());
    let mut corrupt_pdf = office_fixtures::pdf(&["x"]);
    corrupt_pdf.truncate(60);
    /// (name, mime, bytes, expected status, expected text)
    type Case<'a> = (&'a str, &'a str, Vec<u8>, &'a str, Option<&'a str>);
    let cases: Vec<Case<'_>> = vec![
        (
            "회의록.docx",
            "application/zip",
            office_fixtures::docx("회의", &["첨부 본문 검색어"]),
            "ok",
            Some("첨부 본문 검색어"),
        ),
        (
            "slides.pptx",
            "application/zip",
            office_fixtures::pptx(&[("Title", "slide words")]),
            "ok",
            Some("slide words"),
        ),
        (
            "sheet.ods",
            "application/zip",
            office_fixtures::ods("S", &[&["cell words"]]),
            "ok",
            Some("cell words"),
        ),
        (
            "paper.pdf",
            "application/pdf",
            office_fixtures::pdf(&["pdf words"]),
            "ok",
            Some("pdf words"),
        ),
        (
            "notes.txt",
            "text/plain",
            "plain 텍스트\0".as_bytes().to_vec(),
            "ok",
            Some("plain 텍스트"),
        ),
        (
            "broken.pdf",
            "application/pdf",
            corrupt_pdf,
            "corrupt",
            None,
        ),
        (
            "renamed.docx",
            "application/pdf",
            office_fixtures::pdf(&["x"]),
            "unsupported",
            None,
        ),
        (
            "bomb.xlsx",
            "application/zip",
            office_fixtures::docx_bomb(),
            "resource_limit",
            None,
        ),
        (
            "empty.xlsx",
            "application/zip",
            office_fixtures::xlsx("s", &[]),
            "empty",
            None,
        ),
        (
            "doc.hwp",
            "application/x-hwp",
            b"not really hwp".to_vec(),
            "skipped",
            None,
        ),
    ];
    let mut ids = Vec::new();
    for (name, mime, bytes, _, _) in &cases {
        let attachment_id = Uuid::now_v7();
        let key = Uuid::now_v7().to_string();
        storage.put_bytes(&key, bytes.clone()).await.unwrap();
        sqlx::query(
            r#"
            INSERT INTO fvoci.attachments (
                id, workspace_id, document_id, uploader_id, status, name, mime,
                size_bytes, reserved_size_bytes, storage_key, image, scan_status,
                extract_status, completed_at
            )
            VALUES ($1, $2, $3, $4, 'stored', $5, $6, $7, $7, $8, false, 'skipped', $9, now())
            "#,
        )
        .bind(attachment_id)
        .bind(workspace_id)
        .bind(document_id)
        .bind(user_id)
        .bind(name)
        .bind(mime)
        .bind(bytes.len() as i64)
        .bind(&key)
        .bind(fvoci_server::attachments::initial_extract_status(
            name, mime,
        ))
        .execute(&admin)
        .await
        .unwrap();
        ids.push(attachment_id);
    }
    let job = spawn_extract_job(
        ExtractJobSettings {
            extractor_bin: None,
            limits: document_extract_client::Limits::for_tests(),
            office_helper: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))),
            office_limits: OfficeLimits::attachment(),
            poll_interval: Duration::from_millis(100),
            retry_backoff: Duration::from_millis(10),
            #[cfg(feature = "extract-native-tests")]
            test_hang_ms: None,
        },
        app.clone(),
        storage,
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    for ((name, _, _, expected, text), attachment_id) in cases.iter().zip(&ids) {
        let state = loop {
            let state = fetch_extract_state(&app, workspace_id, *attachment_id)
                .await
                .unwrap()
                .unwrap();
            if state.extract_status != "pending" {
                break state;
            }
            assert!(std::time::Instant::now() < deadline, "{name} still pending");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(state.extract_status, *expected, "{name}");
        match text {
            Some(text) => {
                assert!(
                    state.extract_text.contains(text),
                    "{name}: {}",
                    state.extract_text
                );
                assert!(!state.extract_text.contains('\0'));
                let chunks: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM fvoci.attachment_text WHERE attachment_id = $1 AND status = 'ok' AND text <> ''",
                )
                .bind(attachment_id)
                .fetch_one(&admin)
                .await
                .unwrap();
                assert!(chunks >= 1, "{name} has no search chunks");
            }
            None => assert_eq!(state.extract_text, "", "{name}"),
        }
    }
    job.request_shutdown();
    job.join().await.unwrap();
    // Images are never queued for text extraction.
    assert_eq!(
        fvoci_server::attachments::initial_extract_status("photo.png", "image/png"),
        "skipped"
    );
    let _ = std::fs::remove_dir_all(&root);
    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}
