//! SQLite product-router fixture shared by the integrations acceptance targets.
#![allow(dead_code)]
use axum::Router;
use fvoci_server::{
    auth::{password::Keyring, service::SetupInstanceInput, AuthService},
    db::{backend::Backend, migrate, pool, Db},
    http::state::AppState,
    integrations::Integrations,
};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;
pub const ENC_KEYS: &str =
    r#"{"k1":"0101010101010101010101010101010101010101010101010101010101010101"}"#;
pub fn state(auth: Arc<AuthService>, storage_root: PathBuf) -> AppState {
    AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::Off,
        native_engine: None,
        auth,
        branding_name: "FVOCI isolated auth proof".into(),
        public_origin: "http://localhost".into(),
        cookie_secure: false,
        rate_limiter: fvoci_server::http::rate_limit::RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        search_embedder: None,
        markdown: None,
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
        streams: AppState::fresh_streams(),
    }
}

pub struct Fixture {
    pub root: PathBuf,
    pub sqlite: sqlx::SqlitePool,
    pub auth: Arc<AuthService>,
    pub user: Uuid,
    pub workspace: Uuid,
    pub personal: Uuid,
    pub session: Uuid,
    pub token: String,
}
impl Fixture {
    pub async fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("fvoci-integration-family-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("integrations.sqlite");
        migrate::run_sqlite_migrations(&path).await.unwrap();
        let sqlite = pool::connect_sqlite_app(&path, 4).await.unwrap();
        let auth = Arc::new(AuthService {
            db: Db::from_backend(Backend::Sqlite(sqlite.clone())),
            password_keys: Keyring::parse(ENC_KEYS, "k1").unwrap(),
        });
        let (user, workspace, token) = auth
            .setup_instance(
                "Synthetic-Integrations-2026!".into(),
                SetupInstanceInput {
                    email: "integration@example.invalid".into(),
                    given_name: "Fixture".into(),
                    family_name: None,
                    workspace_slug: "integration-fixture".into(),
                    workspace_name: "Integration fixture".into(),
                    client_ip: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
        let live = auth.session_user(&token).await.unwrap().unwrap();
        let session = Uuid::parse_str(&live.session_id).unwrap();
        let personal = fvoci_server::db::workspace::ensure_personal_workspace_backend(
            &auth.db.pool,
            &fvoci_server::license::absent(),
            user,
            session,
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id;
        Self {
            root,
            sqlite,
            auth,
            user,
            workspace,
            personal,
            session,
            token,
        }
    }
    pub fn app(&self, integrations: Arc<Integrations>) -> Router {
        let mut s = state(self.auth.clone(), self.root.join("storage"));
        s.markdown = Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        );
        fvoci_server::http::router_with_integrations(s, None, integrations)
    }
    pub async fn fresh(&self) -> sqlx::SqlitePool {
        pool::connect_sqlite_app(&self.root.join("integrations.sqlite"), 1)
            .await
            .unwrap()
    }

    pub async fn seed_project(&self) -> (Uuid, Uuid, Uuid) {
        let project = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let backlog = Uuid::now_v7();
        let done = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'TEST','Fixture','workspace',?3)").bind(project.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(self.user.as_bytes().as_slice()).execute(&self.sqlite).await.unwrap();
        // Mirror projects_task_collection for this raw-SQL project fixture.
        sqlx::query("INSERT INTO collections(id,workspace_id,project_id,kind,name) VALUES(?1,?2,?3,'task','Fixture')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice()).execute(&self.sqlite).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(self.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&self.sqlite)
            .await
            .unwrap();
        for (id, name, category, sort) in [
            (backlog, "Backlog", "backlog", "a0"),
            (done, "Done", "done", "a1"),
        ] {
            sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,?5,?6,?7)").bind(id.as_bytes().as_slice()).bind(self.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).bind(name).bind(category).bind(sort).execute(&self.sqlite).await.unwrap();
        }
        (project, backlog, done)
    }
    pub async fn cleanup(self) {
        self.auth.db.pool.close().await.unwrap();
        self.sqlite.close().await;
        std::fs::remove_dir_all(self.root).unwrap();
    }
}
