#![cfg(feature = "db-tests")]

use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::collab::derived_body::{prepare_derived_body, PreparedDerivedBody};
use fvoci_server::db::collab::{
    append_collab_update, claim_writer_and_load, compact_collab_snapshot, load_collab_document,
    lookup_collab_operation, project_derived_body, verify_collab_operation, AppendCollabInput,
    AppendCollabResult, CollabDbError, CompactCollabInput, ProjectDerivedBodyInput,
    ProjectDerivedBodyResult, VerifyCollabInput, MAX_COLLAB_SNAPSHOT_BYTES,
    MAX_COLLAB_TAIL_UPDATES, MAX_COLLAB_UPDATE_BYTES,
};
use fvoci_server::db::documents::{empty_document_json, CreateDocumentInput};
use fvoci_server::db::identity::revoke_session;
use fvoci_server::db::workspace::{self, WorkspaceRole};
use fvoci_server::db::{documents, migrate, pool};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

fn payload_digest(payload: &[u8]) -> Vec<u8> {
    Sha256::digest(payload).to_vec()
}

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

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

        let db_name = format!("fvoci_test_{}", Uuid::now_v7().simple());
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

struct SessionFixture {
    pool: PgPool,
    user_id: Uuid,
    session_id: Uuid,
    workspace_id: Uuid,
    token_hash: String,
}

struct WikiDocFixture {
    session: SessionFixture,
    document_id: Uuid,
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
    let quoted_role = format!("\"{}\"", role_name);
    let grants =
        include_str!("../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
    for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.expect("grant");
    }
}

async fn setup_owner_session(harness: &TestDb) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
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
    .bind(format!("owner-{user_id}@example.com"))
    .bind(&hash)
    .bind("Owner")
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(workspace::personal_workspace_slug(user_id))
        .bind("Collab WS")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let token = fvoci_server::auth::token::new_token();
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
        token_hash: token.hash,
    }
}

async fn setup_team_owner_session(harness: &TestDb) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
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
    .bind(format!("team-owner-{user_id}@example.com"))
    .bind(&hash)
    .bind("Team Owner")
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name, kind) VALUES ($1, $2, $3, 'team')")
        .bind(workspace_id)
        .bind(format!(
            "tc{}",
            &workspace_id.simple().to_string().to_lowercase()[..8]
        ))
        .bind("Collab Team WS")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let token = fvoci_server::auth::token::new_token();
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
        token_hash: token.hash,
    }
}

async fn create_wiki_doc(session: &SessionFixture, title: &str) -> Uuid {
    let created = documents::create_wiki_document(
        &session.pool,
        session.workspace_id,
        session.user_id,
        session.session_id,
        CreateDocumentInput {
            parent_id: None,
            title,
            icon: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    created.id
}

async fn setup_wiki_doc(harness: &TestDb) -> WikiDocFixture {
    let session = setup_owner_session(harness).await;
    let document_id = create_wiki_doc(&session, "Collab doc").await;
    WikiDocFixture {
        session,
        document_id,
    }
}

async fn setup_team_wiki_doc(harness: &TestDb) -> WikiDocFixture {
    let session = setup_team_owner_session(harness).await;
    let document_id = create_wiki_doc(&session, "Team collab doc").await;
    WikiDocFixture {
        session,
        document_id,
    }
}

fn append_input<'a>(
    session: &'a SessionFixture,
    document_id: Uuid,
    writer_generation: i64,
    expected_tail_seq: i64,
    op_id: Uuid,
    payload: &'a [u8],
) -> AppendCollabInput<'a> {
    AppendCollabInput {
        workspace_id: session.workspace_id,
        actor_user_id: session.user_id,
        session_id: session.session_id,
        document_id,
        writer_generation,
        expected_tail_seq,
        op_id,
        payload,
        client_ip: None,
    }
}

fn compact_input<'a>(
    session: &'a SessionFixture,
    document_id: Uuid,
    writer_generation: i64,
    cutoff_seq: i64,
    expected_tail_seq: i64,
    new_snapshot: &'a [u8],
) -> CompactCollabInput<'a> {
    CompactCollabInput {
        workspace_id: session.workspace_id,
        actor_user_id: session.user_id,
        session_id: session.session_id,
        document_id,
        writer_generation,
        cutoff_seq,
        expected_tail_seq,
        new_snapshot,
        client_ip: None,
    }
}

fn project_input(
    session: &SessionFixture,
    document_id: Uuid,
    writer_generation: i64,
    expected_tail_seq: i64,
    prepared: &PreparedDerivedBody,
) -> ProjectDerivedBodyInput {
    ProjectDerivedBodyInput::new(
        session.workspace_id,
        session.user_id,
        session.session_id,
        document_id,
        writer_generation,
        expected_tail_seq,
        prepared.clone(),
    )
}

fn derived_doc_json(text: &str) -> Value {
    json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [{"type": "text", "text": text}]
        }]
    })
}

async fn event_count(admin: &PgPool, document_id: Uuid, verb: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*)::bigint
        FROM fvoci.events
        WHERE target_id = $1 AND verb = $2
        "#,
    )
    .bind(document_id)
    .bind(verb)
    .fetch_one(admin)
    .await
    .unwrap();
    row.0
}

async fn create_member_session(
    harness: &TestDb,
    workspace_id: Uuid,
    email: &str,
) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
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
    .bind(email)
    .bind(&hash)
    .bind("Member")
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    workspace::add_membership_for_test(&pool, workspace_id, user_id, WorkspaceRole::Member)
        .await
        .unwrap();
    let token = fvoci_server::auth::token::new_token();
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
        token_hash: token.hash,
    }
}

async fn install_insert_fail_trigger(admin: &PgPool, target: &str, fn_name: &str) {
    sqlx::query(&format!(
        r#"
        CREATE OR REPLACE FUNCTION fvoci.{fn_name}()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            RAISE EXCEPTION 'insert blocked on {target}';
        END;
        $$;
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"
        CREATE TRIGGER fvoci_{fn_name}
        BEFORE INSERT ON fvoci.{target}
        FOR EACH ROW EXECUTE FUNCTION fvoci.{fn_name}()
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
}

async fn wait_for_users_for_update(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_lock_blocked_by(admin, blocker_pid, "%fvoci.users%", "%FOR UPDATE%").await
}

async fn wait_for_lock_sign_in_blocked(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_lock_blocked_by(
        admin,
        blocker_pid,
        "%FROM fvoci.users u%",
        "%FOR UPDATE OF u, s%",
    )
    .await
}

async fn wait_for_lock_blocked_by(
    admin: &PgPool,
    blocker_pid: i32,
    query_like: &str,
    lock_like: &str,
) -> i32 {
    wait_for_lock_blocked_by_any(admin, &[blocker_pid], query_like, lock_like).await
}

async fn wait_for_lock_blocked_by_any(
    admin: &PgPool,
    blocker_pids: &[i32],
    query_like: &str,
    lock_like: &str,
) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE $2
              AND activity.query ILIKE $3
              AND pg_blocking_pids(activity.pid) && $1::integer[]
            LIMIT 1
            ",
        )
        .bind(blocker_pids)
        .bind(query_like)
        .bind(lock_like)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "collab operation did not block on expected lock (query ~ {query_like}, lock ~ {lock_like})"
    );
}

async fn wait_for_advisory_xact_lock_blocked_by(admin: &PgPool, holder_pid: i32) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%pg_advisory_xact_lock%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            ",
        )
        .bind(holder_pid)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("operation did not block on advisory xact lock held by pid {holder_pid}");
}

async fn wait_for_document_states_for_update(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_lock_blocked_by(admin, blocker_pid, "%document_states%", "%FOR UPDATE%").await
}

async fn wait_for_documents_for_update(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_lock_blocked_by(admin, blocker_pid, "%fvoci.documents%", "%FOR UPDATE%").await
}

async fn wait_for_session_revoke_blocked(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_lock_blocked_by(
        admin,
        blocker_pid,
        "%UPDATE fvoci.sessions%",
        "%revoked_at%",
    )
    .await
}

#[tokio::test]
async fn fresh_migration_005_adds_collab_tables_and_columns() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(versions.0, 8);
    let has_updates: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'document_collab_updates')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_updates.0);
    let has_generation: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'fvoci' AND table_name = 'document_states' AND column_name = 'writer_generation')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_generation.0);
    let has_receipts: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'document_collab_op_receipts')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_receipts.0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_004_upgrades_to_005_collab() {
    let admin_base = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing");
    let db_name = format!("fvoci_test_{}", Uuid::now_v7().simple());
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
    ] {
        sqlx::raw_sql(sql).execute(&migration_pool).await.unwrap();
    }
    sqlx::query("INSERT INTO fvoci.schema_migrations (version) VALUES (1), (2), (3), (4)")
        .execute(&migration_pool)
        .await
        .unwrap();
    let has_updates: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'document_collab_updates')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(!has_updates.0);
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
    assert_eq!(versions.0, 8);
    let has_updates: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'document_collab_updates')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_updates.0);

    let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
    let mut password_bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut password_bytes);
    let role_password = hex::encode(password_bytes);
    sqlx::query(&format!(
        "CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{role_password}' NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&migration_pool)
    .await
    .unwrap();
    apply_grants(&migration_pool, &role_name).await;

    let mut app_url = url::Url::parse(&admin_url).unwrap();
    app_url.set_username(&role_name).ok();
    app_url.set_password(Some(&role_password)).ok();
    let app_pool = pool::connect_app(app_url.as_ref()).await.unwrap();

    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    let document_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
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
    .bind(format!("upgrade-{user_id}@example.com"))
    .bind(&hash)
    .bind("Upgrade")
    .execute(&migration_pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(workspace::personal_workspace_slug(user_id))
        .bind("Upgrade WS")
        .execute(&migration_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&migration_pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Upgrade doc', $3, 'V', 1, 'draft', 2, $4::jsonb, $5
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(document_id.simple().to_string())
    .bind(empty_document_json())
    .bind(user_id)
    .execute(&migration_pool)
    .await
    .unwrap();

    let token = fvoci_server::auth::token::new_token();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = app_pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let claim = claim_writer_and_load(&app_pool, workspace_id, user_id, session_id, document_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.writer_generation, 1);

    let can_update: (bool,) = sqlx::query_as(
        "SELECT has_table_privilege(current_user, 'fvoci.document_collab_op_receipts', 'UPDATE')",
    )
    .fetch_one(&app_pool)
    .await
    .unwrap();
    let can_delete: (bool,) = sqlx::query_as(
        "SELECT has_table_privilege(current_user, 'fvoci.document_collab_op_receipts', 'DELETE')",
    )
    .fetch_one(&app_pool)
    .await
    .unwrap();
    assert!(!can_update.0);
    assert!(!can_delete.0);

    app_pool.close().await;
    migration_pool.close().await;

    let server = server_db_url(&admin_url);
    let cleanup_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server)
        .await
        .unwrap();
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{db_name}'"
    ))
    .execute(&cleanup_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db_name}\""))
        .execute(&cleanup_pool)
        .await;
    let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{role_name}\""))
        .execute(&cleanup_pool)
        .await;
    cleanup_pool.close().await;
}

#[tokio::test]
async fn claim_seeds_empty_yjs_and_loads_snapshot_with_tail() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(claim.writer_generation, 1);
    assert_eq!(claim.load.snapshot, vec![0, 0]);
    assert!(claim.load.tail.is_empty());
    assert_eq!(claim.load.snapshot_cutoff_seq, 0);
    assert_eq!(claim.load.tail_seq, 0);

    let op_id = Uuid::now_v7();
    let payload = b"update-one";
    let append = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            payload,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(append, AppendCollabResult::Committed { seq: 1 });

    let load = load_collab_document(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(load.tail.len(), 1);
    assert_eq!(load.tail[0].op_id, op_id);
    assert_eq!(load.tail[0].payload, payload);
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn stale_writer_generation_is_rejected_after_takeover() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let first = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let second = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(second.writer_generation, first.writer_generation + 1);

    let stale = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            first.writer_generation,
            0,
            Uuid::now_v7(),
            b"stale",
        ),
    )
    .await
    .unwrap();
    assert_eq!(stale, Err(CollabDbError::StaleWriter));

    let ok = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            second.writer_generation,
            0,
            Uuid::now_v7(),
            b"fresh",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ok, AppendCollabResult::Committed { seq: 1 });
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn duplicate_op_id_same_bytes_ack_different_bytes_conflict() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let op_id = Uuid::now_v7();
    let payload = b"same-bytes";
    let first = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            payload,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first, AppendCollabResult::Committed { seq: 1 });

    let dup = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            payload,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(dup, AppendCollabResult::DuplicateAck { seq: 1 });

    let conflict = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            b"different",
        ),
    )
    .await
    .unwrap();
    assert_eq!(conflict, Err(CollabDbError::OpIdConflict));

    let updates: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .fetch_one(
        &PgPoolOptions::new()
            .max_connections(1)
            .connect(&harness.admin_url)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(updates.0, 1);
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn compact_rejects_oversized_snapshot() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"one",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let huge = vec![1u8; MAX_COLLAB_SNAPSHOT_BYTES + 1];
    let err = compact_collab_snapshot(
        &fixture.session.pool,
        compact_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            1,
            &huge,
        ),
    )
    .await
    .unwrap();
    assert_eq!(err, Err(CollabDbError::PayloadTooLarge));
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn append_rejects_oversized_payload() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let huge = vec![1u8; MAX_COLLAB_UPDATE_BYTES + 1];
    let err = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            &huge,
        ),
    )
    .await
    .unwrap();
    assert_eq!(err, Err(CollabDbError::PayloadTooLarge));
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_loses_to_session_revoke_barrier() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let mut barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.sessions WHERE id = $1 FOR UPDATE")
        .bind(fixture.session.session_id)
        .execute(&mut *barrier)
        .await
        .unwrap();

    let revoke = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let token_hash = fixture.session.token_hash.clone();
        async move { revoke_session(&pool, &token_hash, None).await }
    });
    let revoke_pid = wait_for_session_revoke_blocked(&admin, blocker_pid).await;

    let append = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let actor_user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 0,
                    op_id: Uuid::now_v7(),
                    payload: b"after-revoke-wins",
                    client_ip: None,
                },
            )
            .await
        }
    });
    wait_for_lock_blocked_by_any(
        &admin,
        &[revoke_pid, blocker_pid],
        "%FROM fvoci.users u%",
        "%FOR UPDATE OF u, s%",
    )
    .await;
    barrier.commit().await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), revoke)
        .await
        .expect("revoke_session did not finish after barrier release")
        .unwrap()
        .unwrap();
    let append_result = tokio::time::timeout(Duration::from_secs(10), append)
        .await
        .expect("append did not finish after revoke")
        .unwrap()
        .unwrap();
    assert_eq!(append_result, Err(CollabDbError::Forbidden));
    let updates: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(updates.0, 0);
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_wins_before_session_revoke_barrier() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let mut barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(fixture.session.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();

    let append = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let actor_user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 0,
                    op_id: Uuid::now_v7(),
                    payload: b"before-revoke",
                    client_ip: None,
                },
            )
            .await
        }
    });
    let append_pid = wait_for_lock_sign_in_blocked(&admin, blocker_pid).await;

    let revoke = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let token_hash = fixture.session.token_hash.clone();
        async move { revoke_session(&pool, &token_hash, None).await }
    });
    wait_for_lock_blocked_by_any(
        &admin,
        &[blocker_pid, append_pid],
        "%FROM fvoci.users WHERE id = $1 FOR UPDATE%",
        "%FOR UPDATE%",
    )
    .await;
    barrier.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), append)
        .await
        .expect("append did not finish after barrier release")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result, AppendCollabResult::Committed { seq: 1 });

    tokio::time::timeout(Duration::from_secs(10), revoke)
        .await
        .expect("revoke_session did not finish after append won")
        .unwrap()
        .unwrap();
    let denied = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"after-revoke",
        ),
    )
    .await
    .unwrap();
    assert_eq!(denied, Err(CollabDbError::Forbidden));
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_wins_before_membership_remove_barrier() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_team_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "member@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let mut state_barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *state_barrier)
        .await
        .unwrap();
    sqlx::query(
        r#"
        SELECT writer_generation
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        FOR UPDATE
        "#,
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .execute(&mut *state_barrier)
    .await
    .unwrap();

    let append = tokio::spawn({
        let pool = member.pool.clone();
        let workspace_id = member.workspace_id;
        let actor_user_id = member.user_id;
        let session_id = member.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 0,
                    op_id: Uuid::now_v7(),
                    payload: b"before-remove",
                    client_ip: None,
                },
            )
            .await
        }
    });
    let append_pid = wait_for_document_states_for_update(&admin, blocker_pid).await;
    let remove = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let owner_id = fixture.session.user_id;
        let owner_session = fixture.session.session_id;
        let member_id = member.user_id;
        async move {
            workspace::remove_member(
                &pool,
                workspace_id,
                owner_id,
                owner_session,
                member_id,
                None,
            )
            .await
        }
    });
    wait_for_advisory_xact_lock_blocked_by(&admin, append_pid).await;
    state_barrier.commit().await.unwrap();

    let append_result = tokio::time::timeout(Duration::from_secs(10), append)
        .await
        .expect("append did not finish after document_states release")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(append_result, AppendCollabResult::Committed { seq: 1 });

    tokio::time::timeout(Duration::from_secs(10), remove)
        .await
        .expect("remove_member did not finish after append won")
        .unwrap()
        .unwrap()
        .unwrap();

    let denied = append_collab_update(
        &member.pool,
        AppendCollabInput {
            workspace_id: member.workspace_id,
            actor_user_id: member.user_id,
            session_id: member.session_id,
            document_id: fixture.document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: 0,
            op_id: Uuid::now_v7(),
            payload: b"after-remove",
            client_ip: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(denied, Err(CollabDbError::Forbidden));
    admin.close().await;
    member.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_loses_to_membership_remove_barrier() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_team_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "drop@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let mut owner_barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *owner_barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(fixture.session.user_id)
        .execute(&mut *owner_barrier)
        .await
        .unwrap();

    let remove = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let owner_id = fixture.session.user_id;
        let owner_session = fixture.session.session_id;
        let member_id = member.user_id;
        async move {
            workspace::remove_member(
                &pool,
                workspace_id,
                owner_id,
                owner_session,
                member_id,
                None,
            )
            .await
        }
    });
    let remove_pid = wait_for_users_for_update(&admin, blocker_pid).await;
    let append = tokio::spawn({
        let pool = member.pool.clone();
        let workspace_id = member.workspace_id;
        let actor_user_id = member.user_id;
        let session_id = member.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 0,
                    op_id: Uuid::now_v7(),
                    payload: b"after-remove-wins",
                    client_ip: None,
                },
            )
            .await
        }
    });
    wait_for_advisory_xact_lock_blocked_by(&admin, remove_pid).await;
    owner_barrier.commit().await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), remove)
        .await
        .expect("remove_member did not finish after owner row release")
        .unwrap()
        .unwrap()
        .unwrap();
    let append_result = tokio::time::timeout(Duration::from_secs(10), append)
        .await
        .expect("append did not finish after removal")
        .unwrap()
        .unwrap();
    assert_eq!(append_result, Err(CollabDbError::Forbidden));
    let updates: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(updates.0, 0);
    admin.close().await;
    member.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_rejects_archived_and_suspended_and_role_demotion() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "arch@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.documents SET status = 'archived' WHERE workspace_id = $1 AND id = $2",
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .execute(&admin)
    .await
    .unwrap();
    let archived = append_collab_update(
        &member.pool,
        AppendCollabInput {
            workspace_id: member.workspace_id,
            actor_user_id: member.user_id,
            session_id: member.session_id,
            document_id: fixture.document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: 0,
            op_id: Uuid::now_v7(),
            payload: b"nope",
            client_ip: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(archived, Err(CollabDbError::Forbidden));

    sqlx::query("UPDATE fvoci.documents SET status = 'draft' WHERE workspace_id = $1 AND id = $2")
        .bind(fixture.session.workspace_id)
        .bind(fixture.document_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let suspended = append_collab_update(
        &member.pool,
        AppendCollabInput {
            workspace_id: member.workspace_id,
            actor_user_id: member.user_id,
            session_id: member.session_id,
            document_id: fixture.document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: 0,
            op_id: Uuid::now_v7(),
            payload: b"nope",
            client_ip: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(suspended, Err(CollabDbError::Forbidden));

    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'guest' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(member.workspace_id)
    .bind(member.user_id)
    .execute(&admin)
    .await
    .unwrap();
    let guest = append_collab_update(
        &member.pool,
        AppendCollabInput {
            workspace_id: member.workspace_id,
            actor_user_id: member.user_id,
            session_id: member.session_id,
            document_id: fixture.document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: 0,
            op_id: Uuid::now_v7(),
            payload: b"nope",
            client_ip: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(guest, Err(CollabDbError::Forbidden));
    admin.close().await;
    member.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lookup_hides_operation_from_forbidden_actor() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "lookup@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let op_id = Uuid::now_v7();
    append_collab_update(
        &member.pool,
        AppendCollabInput {
            workspace_id: member.workspace_id,
            actor_user_id: member.user_id,
            session_id: member.session_id,
            document_id: fixture.document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: 0,
            op_id,
            payload: b"secret",
            client_ip: None,
        },
    )
    .await
    .unwrap()
    .unwrap();

    let other_ws = Uuid::now_v7();
    let stranger = setup_owner_session(&harness).await;
    let hidden = lookup_collab_operation(
        &stranger.pool,
        other_ws,
        stranger.user_id,
        stranger.session_id,
        fixture.document_id,
        op_id,
    )
    .await
    .unwrap();
    assert_eq!(hidden, Err(CollabDbError::NotFound));

    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'guest' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(member.workspace_id)
    .bind(member.user_id)
    .execute(
        &PgPoolOptions::new()
            .max_connections(1)
            .connect(&harness.admin_url)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let guest_lookup = lookup_collab_operation(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
        op_id,
    )
    .await
    .unwrap();
    assert_eq!(guest_lookup, Err(CollabDbError::NotFound));

    let owner_lookup = lookup_collab_operation(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
        op_id,
    )
    .await
    .unwrap()
    .unwrap()
    .expect("owner can see op");
    assert_eq!(owner_lookup.payload_len, b"secret".len() as i64);
    assert_eq!(owner_lookup.payload_sha256, payload_digest(b"secret"));
    assert_eq!(owner_lookup.actor_user_id, member.user_id);
    member.pool.close().await;
    stranger.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn event_or_audit_failure_rolls_back_append_and_seq() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_collab_event_fail").await;
    let failed = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"rollback-me",
        ),
    )
    .await;
    assert!(failed.is_err());
    let updates: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(updates.0, 0);
    let tail_seq: (i64,) =
        sqlx::query_as("SELECT tail_seq FROM fvoci.document_states WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(tail_seq.0, 0);
    sqlx::query("DROP TRIGGER fvoci_test_collab_event_fail ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    install_insert_fail_trigger(&admin, "audit_log", "test_collab_audit_fail").await;
    let failed = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"rollback-me",
        ),
    )
    .await;
    assert!(failed.is_err());
    let updates: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(updates.0, 0);
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pool_tenant_context_resets_after_collab_commit_and_rollback() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let other_ws = Uuid::now_v7();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other', 'Other')")
        .bind(other_ws)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"ctx",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let mut tx = fixture.session.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(other_ws.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let hidden: Option<(Uuid,)> =
        sqlx::query_as("SELECT document_id FROM fvoci.document_collab_updates LIMIT 1")
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(hidden.is_none());
    tx.rollback().await.unwrap();

    let mut tx = fixture.session.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(fixture.session.workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible: Option<(Uuid,)> =
        sqlx::query_as("SELECT document_id FROM fvoci.document_collab_updates LIMIT 1")
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(visible.is_some());
    tx.commit().await.unwrap();
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_cannot_update_or_delete_collab_receipts() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let op_id = Uuid::now_v7();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            b"immutable-receipt",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    for (sql, label) in [
        (
            r#"
            UPDATE fvoci.document_collab_op_receipts
            SET payload_len = 99
            WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
            "#,
            "receipt update",
        ),
        (
            r#"
            DELETE FROM fvoci.document_collab_op_receipts
            WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
            "#,
            "receipt delete",
        ),
    ] {
        let mut tx = fixture.session.pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(fixture.session.workspace_id.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        let err = sqlx::query(sql)
            .bind(fixture.session.workspace_id)
            .bind(fixture.document_id)
            .bind(op_id)
            .execute(&mut *tx)
            .await
            .expect_err(label);
        assert_eq!(
            err.as_database_error()
                .and_then(|e| e.code())
                .map(|c| c.to_string()),
            Some("42501".to_string())
        );
        tx.rollback().await.unwrap();
    }

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let stored_len: (i64,) = sqlx::query_as(
        r#"
        SELECT payload_len
        FROM fvoci.document_collab_op_receipts
        WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
        "#,
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .bind(op_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(stored_len.0, b"immutable-receipt".len() as i64);
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_cannot_insert_foreign_tenant_collab_update() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let other_ws = Uuid::now_v7();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'foreign', 'Foreign')")
        .bind(other_ws)
        .execute(&admin)
        .await
        .unwrap();
    let foreign_doc = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Foreign', $3, 'V', 1, 'draft', 2, $4::jsonb, $5
        )
        "#,
    )
    .bind(foreign_doc)
    .bind(other_ws)
    .bind(foreign_doc.simple().to_string())
    .bind(empty_document_json())
    .bind(fixture.session.user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let mut tx = fixture.session.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(fixture.session.workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query(
        r#"
        INSERT INTO fvoci.document_collab_updates (
            workspace_id, document_id, seq, op_id, payload
        ) VALUES ($1, $2, 1, $3, '\x01')
        "#,
    )
    .bind(other_ws)
    .bind(foreign_doc)
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant insert must fail");
    assert_eq!(
        err.as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.to_string()),
        Some("42501".to_string())
    );
    tx.rollback().await.unwrap();
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn compact_snapshot_requires_exact_cutoff_and_rejects_stale_fence() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"one",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            Uuid::now_v7(),
            b"two",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let stale_fence = compact_collab_snapshot(
        &fixture.session.pool,
        compact_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            1,
            b"merged",
        ),
    )
    .await
    .unwrap();
    assert_eq!(stale_fence, Err(CollabDbError::StaleCutoff));

    let merged = compact_collab_snapshot(
        &fixture.session.pool,
        compact_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            2,
            2,
            b"merged-at-2",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(merged.snapshot, b"merged-at-2");
    assert_eq!(merged.snapshot_cutoff_seq, 2);
    assert!(merged.tail.is_empty());

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let remaining: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(remaining.0, 0);
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn claim_blocks_inflight_append_until_generation_bump_resolves() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let first = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let mut hold = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    sqlx::query(
        r#"
        SELECT writer_generation
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        FOR UPDATE
        "#,
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .execute(&mut *hold)
    .await
    .unwrap();

    let append = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let actor_user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        let writer_generation = first.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 0,
                    op_id: Uuid::now_v7(),
                    payload: b"race",
                    client_ip: None,
                },
            )
            .await
        }
    });
    wait_for_document_states_for_update(&admin, blocker_pid).await;
    let claim = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        async move { claim_writer_and_load(&pool, workspace_id, user_id, session_id, document_id).await }
    });
    wait_for_document_states_for_update(&admin, blocker_pid).await;

    hold.commit().await.unwrap();
    let append_result = append.await.unwrap().unwrap().unwrap();
    assert_eq!(append_result, AppendCollabResult::Committed { seq: 1 });
    let takeover = claim.await.unwrap().unwrap().unwrap();
    assert!(takeover.writer_generation > first.writer_generation);

    let stale = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            first.writer_generation,
            0,
            Uuid::now_v7(),
            b"stale",
        ),
    )
    .await
    .unwrap();
    assert_eq!(stale, Err(CollabDbError::StaleWriter));
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn compact_preserves_receipt_lookup_and_duplicate_ack() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let op_id = Uuid::now_v7();
    let payload = b"kept-after-compact";
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            payload,
        ),
    )
    .await
    .unwrap()
    .unwrap();

    compact_collab_snapshot(
        &fixture.session.pool,
        compact_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            1,
            b"merged",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let lookup = lookup_collab_operation(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
        op_id,
    )
    .await
    .unwrap()
    .unwrap()
    .expect("receipt survives compaction");
    assert_eq!(lookup.payload_len, payload.len() as i64);
    assert_eq!(lookup.payload_sha256, payload_digest(payload));
    assert_eq!(lookup.actor_user_id, fixture.session.user_id);

    let dup = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            payload,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(dup, AppendCollabResult::DuplicateAck { seq: 1 });
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn append_rejects_state_budget_exhaustion() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let row_payload = b"x";
    for seq in 1..=MAX_COLLAB_TAIL_UPDATES {
        sqlx::query(
            r#"
            INSERT INTO fvoci.document_collab_updates (
                workspace_id, document_id, seq, op_id, payload
            ) VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(fixture.session.workspace_id)
        .bind(fixture.document_id)
        .bind(seq)
        .bind(Uuid::now_v7())
        .bind(row_payload)
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        UPDATE fvoci.document_states
        SET tail_seq = $3
        WHERE workspace_id = $1 AND document_id = $2
        "#,
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .bind(MAX_COLLAB_TAIL_UPDATES)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let err = append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            MAX_COLLAB_TAIL_UPDATES,
            Uuid::now_v7(),
            b"x",
        ),
    )
    .await
    .unwrap();
    assert_eq!(err, Err(CollabDbError::StateBudgetExceeded));
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn verify_collab_operation_rejects_payload_and_actor_mismatch() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "verify@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let op_id = Uuid::now_v7();
    append_collab_update(
        &member.pool,
        append_input(
            &member,
            fixture.document_id,
            claim.writer_generation,
            0,
            op_id,
            b"truth",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let truth = b"truth";
    let truth_digest = payload_digest(truth);
    let bad_payload = verify_collab_operation(
        &fixture.session.pool,
        VerifyCollabInput {
            workspace_id: fixture.session.workspace_id,
            actor_user_id: fixture.session.user_id,
            session_id: fixture.session.session_id,
            document_id: fixture.document_id,
            op_id,
            expected_payload_len: truth.len() as i64,
            expected_payload_sha256: &payload_digest(b"lie"),
            expected_actor_user_id: member.user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(bad_payload, Err(CollabDbError::OpIdConflict));

    let bad_actor = verify_collab_operation(
        &fixture.session.pool,
        VerifyCollabInput {
            workspace_id: fixture.session.workspace_id,
            actor_user_id: fixture.session.user_id,
            session_id: fixture.session.session_id,
            document_id: fixture.document_id,
            op_id,
            expected_payload_len: truth.len() as i64,
            expected_payload_sha256: &truth_digest,
            expected_actor_user_id: fixture.session.user_id,
        },
    )
    .await
    .unwrap();
    assert_eq!(bad_actor, Err(CollabDbError::OpIdConflict));

    let ok = verify_collab_operation(
        &fixture.session.pool,
        VerifyCollabInput {
            workspace_id: fixture.session.workspace_id,
            actor_user_id: fixture.session.user_id,
            session_id: fixture.session.session_id,
            document_id: fixture.document_id,
            op_id,
            expected_payload_len: truth.len() as i64,
            expected_payload_sha256: &truth_digest,
            expected_actor_user_id: member.user_id,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ok.payload_len, truth.len() as i64);
    assert_eq!(ok.payload_sha256, truth_digest);
    assert_eq!(ok.actor_user_id, member.user_id);
    member.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_fence_rejects_stale_generation_and_tail_seq() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"one",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("projected")).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let stale_tail = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(stale_tail, Err(CollabDbError::StaleCutoff));

    let stale_gen = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation + 1,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(stale_gen, Err(CollabDbError::StaleWriter));

    let before_events = event_count(&admin, fixture.document_id, "document.updated").await;
    let updated = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated, ProjectDerivedBodyResult::Updated);
    let after_events = event_count(&admin, fixture.document_id, "document.updated").await;
    assert_eq!(after_events, before_events + 1);

    let unchanged = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(unchanged, ProjectDerivedBodyResult::Unchanged);
    let final_events = event_count(&admin, fixture.document_id, "document.updated").await;
    assert_eq!(final_events, after_events);

    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_skips_seed_at_tail_seq_zero() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(claim.load.tail_seq, 0);

    let prepared = prepare_derived_body(json!({"type":"doc","content":[]})).unwrap();
    let skipped = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            &prepared,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(skipped, ProjectDerivedBodyResult::SkippedSeed);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let content: (Value,) =
        sqlx::query_as("SELECT content_json FROM fvoci.documents WHERE id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(content.0, empty_document_json());
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_denies_archived_and_trashed_documents() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"one",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("blocked")).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let baseline: (Value, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT content_json, updated_at FROM fvoci.documents WHERE id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();

    sqlx::query(
        "UPDATE fvoci.documents SET status = 'archived' WHERE workspace_id = $1 AND id = $2",
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .execute(&admin)
    .await
    .unwrap();
    let archived = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(archived, Err(CollabDbError::Forbidden));

    sqlx::query(
        "UPDATE fvoci.documents SET status = 'draft', deleted_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(fixture.session.workspace_id)
    .bind(fixture.document_id)
    .execute(&admin)
    .await
    .unwrap();
    let trashed = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(trashed, Err(CollabDbError::NotFound));

    let events = event_count(&admin, fixture.document_id, "document.updated").await;
    assert_eq!(events, 0);
    let after: (Value, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT content_json, updated_at FROM fvoci.documents WHERE id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(after, baseline);
    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_event_failure_rolls_back_content_write() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"durability",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_derived_event_fail").await;

    let prepared = prepare_derived_body(derived_doc_json("rollback")).unwrap();
    let failed = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await;
    assert!(failed.is_err());

    let content: (Value,) =
        sqlx::query_as("SELECT content_json FROM fvoci.documents WHERE id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(content.0, empty_document_json());

    let tail_seq: (i64,) =
        sqlx::query_as("SELECT tail_seq FROM fvoci.document_states WHERE document_id = $1")
            .bind(fixture.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(tail_seq.0, 1);

    sqlx::query("DROP TRIGGER fvoci_test_derived_event_fail ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    let retry = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(retry, ProjectDerivedBodyResult::Updated);

    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_app_role_rls_and_system_channel_event() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let other = setup_owner_session(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"tenant-a",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("tenant scoped")).unwrap();
    let updated = project_derived_body(
        &fixture.session.pool,
        project_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updated, ProjectDerivedBodyResult::Updated);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let event: (Option<Uuid>, String) = sqlx::query_as(
        r#"
        SELECT actor_user_id, channel
        FROM fvoci.events
        WHERE target_id = $1 AND verb = 'document.updated'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(fixture.document_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(event.0.is_none());
    assert_eq!(event.1, "system");

    let prepared_cross = prepare_derived_body(derived_doc_json("cross tenant")).unwrap();
    let denied = project_derived_body(
        &other.pool,
        ProjectDerivedBodyInput::new(
            other.workspace_id,
            other.user_id,
            other.session_id,
            fixture.document_id,
            claim.writer_generation,
            1,
            prepared_cross,
        ),
    )
    .await
    .unwrap();
    assert_eq!(denied, Err(CollabDbError::NotFound));

    let version: (i32,) = sqlx::query_as("SELECT version FROM fvoci.documents WHERE id = $1")
        .bind(fixture.document_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(version.0, 1);

    admin.close().await;
    other.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_acl_denies_guest_and_revoked_session() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let member =
        create_member_session(&harness, fixture.session.workspace_id, "derive@example.com").await;
    let claim = claim_writer_and_load(
        &member.pool,
        member.workspace_id,
        member.user_id,
        member.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &member.pool,
        append_input(
            &member,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"member",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("member write")).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'guest' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(member.workspace_id)
    .bind(member.user_id)
    .execute(&admin)
    .await
    .unwrap();
    let guest_denied = project_derived_body(
        &member.pool,
        project_input(
            &member,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(guest_denied, Err(CollabDbError::Forbidden));

    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'member' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(member.workspace_id)
    .bind(member.user_id)
    .execute(&admin)
    .await
    .unwrap();
    let _ = revoke_session(&member.pool, &member.token_hash, None).await;
    let revoked = project_derived_body(
        &member.pool,
        project_input(
            &member,
            fixture.document_id,
            claim.writer_generation,
            1,
            &prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(revoked, Err(CollabDbError::Forbidden));

    admin.close().await;
    member.pool.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_denies_workspace_spoof_from_non_member() {
    let harness = TestDb::bootstrap().await;
    let victim = setup_wiki_doc(&harness).await;
    let attacker = setup_owner_session(&harness).await;
    let claim = claim_writer_and_load(
        &victim.session.pool,
        victim.session.workspace_id,
        victim.session.user_id,
        victim.session.session_id,
        victim.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &victim.session.pool,
        append_input(
            &victim.session,
            victim.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"victim",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let baseline: (Value, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT content_json, updated_at FROM fvoci.documents WHERE id = $1")
            .bind(victim.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("spoofed")).unwrap();
    let denied = project_derived_body(
        &attacker.pool,
        ProjectDerivedBodyInput::new(
            victim.session.workspace_id,
            attacker.user_id,
            attacker.session_id,
            victim.document_id,
            claim.writer_generation,
            1,
            prepared,
        ),
    )
    .await
    .unwrap();
    assert_eq!(denied, Err(CollabDbError::Forbidden));

    let after: (Value, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT content_json, updated_at FROM fvoci.documents WHERE id = $1")
            .bind(victim.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(after, baseline);
    let events = event_count(&admin, victim.document_id, "document.updated").await;
    assert_eq!(events, 0);

    admin.close().await;
    attacker.pool.close().await;
    victim.session.pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn derived_body_stale_projection_race_denies_after_newer_append() {
    let harness = TestDb::bootstrap().await;
    let fixture = setup_wiki_doc(&harness).await;
    let claim = claim_writer_and_load(
        &fixture.session.pool,
        fixture.session.workspace_id,
        fixture.session.user_id,
        fixture.session.session_id,
        fixture.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    append_collab_update(
        &fixture.session.pool,
        append_input(
            &fixture.session,
            fixture.document_id,
            claim.writer_generation,
            0,
            Uuid::now_v7(),
            b"first",
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let mut doc_barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *doc_barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE")
        .bind(fixture.session.workspace_id)
        .bind(fixture.document_id)
        .execute(&mut *doc_barrier)
        .await
        .unwrap();

    let prepared = prepare_derived_body(derived_doc_json("stale race")).unwrap();
    let append = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let actor_user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        async move {
            append_collab_update(
                &pool,
                AppendCollabInput {
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    expected_tail_seq: 1,
                    op_id: Uuid::now_v7(),
                    payload: b"second",
                    client_ip: None,
                },
            )
            .await
        }
    });
    let append_pid = wait_for_documents_for_update(&admin, blocker_pid).await;

    let project = tokio::spawn({
        let pool = fixture.session.pool.clone();
        let workspace_id = fixture.session.workspace_id;
        let actor_user_id = fixture.session.user_id;
        let session_id = fixture.session.session_id;
        let document_id = fixture.document_id;
        let writer_generation = claim.writer_generation;
        let prepared = prepared.clone();
        async move {
            project_derived_body(
                &pool,
                ProjectDerivedBodyInput::new(
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    writer_generation,
                    1,
                    prepared,
                ),
            )
            .await
        }
    });
    wait_for_lock_blocked_by_any(
        &admin,
        &[blocker_pid, append_pid],
        "%fvoci.documents%",
        "%FOR UPDATE%",
    )
    .await;
    doc_barrier.commit().await.unwrap();

    let append_result = tokio::time::timeout(Duration::from_secs(10), append)
        .await
        .expect("append did not finish after documents release")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(append_result, AppendCollabResult::Committed { seq: 2 });

    let project_result = tokio::time::timeout(Duration::from_secs(10), project)
        .await
        .expect("project did not finish after documents release")
        .unwrap()
        .unwrap();
    assert_eq!(project_result, Err(CollabDbError::StaleCutoff));

    admin.close().await;
    fixture.session.pool.close().await;
    harness.cleanup().await;
}
