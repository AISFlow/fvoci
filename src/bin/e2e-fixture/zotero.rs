//! Explicit db-tests fixture command, never a production endpoint or fallback.
use fvoci_server::{
    auth::{password::Keyring, AuthService},
    collab::{CollabConfig, CollabHub},
    db::{migrate, pool, Db},
    http::{rate_limit::RateLimiter, state::AppState},
    integrations::{zotero::fixtures::Upstream, Integrations},
};
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
async fn output(value: serde_json::Value) -> std::io::Result<()> {
    let mut out = tokio::io::stdout();
    out.write_all(format!("{value}\n").as_bytes()).await?;
    out.flush().await
}

pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    collab_engine::process::make_process_non_dumpable()?;
    let app_url = std::env::var("DATABASE_APP_URL")?;
    let pool = pool::connect_app_with_max(&app_url, 16).await?;
    migrate::assert_app_role(&pool).await?;
    migrate::assert_schema_current(&pool).await?;
    let mut cfg = CollabConfig::from_env().ok_or("fixture requires its built collab-engine")?;
    cfg.max_rooms = 2;
    cfg.max_child_concurrency = fvoci_server::collab::config::derive_max_child_concurrency(2);
    let hub = Arc::new(CollabHub::new(cfg, pool.clone()));
    let native = PathBuf::from(std::env::var("FVOCI_E2E_SERVER_BIN")?);
    let dist = PathBuf::from(std::env::var("FVOCI_E2E_DIST")?);
    if !native.is_file() || !dist.join("index.html").is_file() {
        return Err("fresh native server and dist are required".into());
    }
    // A caller may lend the object store of an installation it owns (e.g. a
    // restored destination); the fixture uses it and never removes it.
    // Unset, the fixture creates and removes its own temporary store.
    let borrowed = std::env::var_os("FVOCI_E2E_ZOTERO_STORAGE_DIR").map(PathBuf::from);
    let storage = match &borrowed {
        Some(dir) => {
            let kind = tokio::fs::symlink_metadata(dir).await?.file_type();
            if !dir.is_absolute() || !kind.is_dir() {
                return Err("borrowed storage must be an existing absolute directory".into());
            }
            dir.clone()
        }
        None => {
            let storage =
                std::env::temp_dir().join(format!("fvoci-zotero-fixture-{}", uuid::Uuid::now_v7()));
            tokio::fs::create_dir(&storage).await?;
            storage
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let keys = Keyring::parse(PEPPER, "test")?;
    let meili = fvoci_server::search::meili::meili_config_from_env()?.map(|config| {
        fvoci_server::search::meili::MeiliConfig::new(
            config.url.clone(),
            config.api_key().to_owned(),
            format!("w6_browser_{}", uuid::Uuid::now_v7().simple()),
        )
    });
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool.clone()),
            password_keys: keys.clone(),
        }),
        branding_name: "FVOCI".into(),
        public_origin: origin.clone(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage.clone()).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: Some(hub.clone()),
        meili: meili.clone(),
        search_embedder: None,
        markdown: Some(fvoci_server::documents::markdown_helper::MarkdownHelper::new(native)),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: AppState::fresh_streams(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
    };
    let upstream = Upstream::start().await;
    let mut integrations = Integrations::disabled();
    integrations.encryption_keys = Some(Arc::new(keys));
    integrations.zotero = upstream.reader();
    let router =
        fvoci_server::http::router_with_integrations(state, Some(dist), Arc::new(integrations));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            stopped.await.ok();
        })
        .await
    });
    output(serde_json::json!({"origin":origin})).await?;
    let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut index_cleanup_required = false;
    while let Some(line) = input.next_line().await? {
        let command: serde_json::Value = serde_json::from_str(&line)?;
        match command["command"].as_str() {
            Some("mode") => {
                let mode = command["mode"]
                    .as_u64()
                    .filter(|m| *m <= 15 || *m == 22)
                    .ok_or("invalid fixture mode")?;
                upstream.model.mode.store(mode as u8, Ordering::SeqCst);
                output(serde_json::json!({"ok":true})).await?;
            }
            Some("requests") => {
                let requests = upstream.model.log.lock().unwrap().clone();
                output(serde_json::json!({"requests":requests})).await?;
            }
            Some("index") => {
                let workspace: uuid::Uuid = command["workspaceId"]
                    .as_str()
                    .ok_or("tenant required")?
                    .parse()?;
                let config = meili.as_ref().ok_or("isolated real Meili required")?;
                index_cleanup_required = true;
                let indexed = fvoci_server::search::index::rebuild_search_index(
                    &pool,
                    config,
                    Some(workspace),
                )
                .await?;
                output(serde_json::json!({"indexedWorkspaces":indexed.workspaces,"pages":indexed.pages})).await?;
            }
            Some("observe") => {
                use sqlx::Connection;
                let user: uuid::Uuid =
                    command["userId"].as_str().ok_or("user required")?.parse()?;
                let workspace: uuid::Uuid = command["workspaceId"]
                    .as_str()
                    .ok_or("tenant required")?
                    .parse()?;
                let connector: uuid::Uuid = command["connectorId"]
                    .as_str()
                    .ok_or("connector required")?
                    .parse()?;
                let mut conn = sqlx::PgConnection::connect(&app_url).await?;
                let flags: (bool, bool) = sqlx::query_as(
                    "SELECT rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
                )
                .fetch_one(&mut conn)
                .await?;
                if flags != (false, false) {
                    return Err("fixture observer must use restricted app role".into());
                }
                sqlx::query("BEGIN").execute(&mut conn).await?;
                sqlx::query("SELECT set_config('app.tenant_id',$1,true),set_config('app.self_user_id',$2,true)").bind(workspace.to_string()).bind(user.to_string()).execute(&mut conn).await?;
                let rows:serde_json::Value=sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_object('id',r.id,'documentId',r.document_id,'itemKey',r.item_key,'title',r.bibliography->>'title','availability',r.availability,'completedVersion',c.completed_version::text,'edges',(SELECT count(*) FROM fvoci.zotero_links l WHERE l.reference_id=r.id)) ORDER BY r.item_key),'[]'::jsonb) FROM fvoci.zotero_references r JOIN fvoci.zotero_connectors c ON c.id=r.connector_id WHERE r.workspace_id=$1 AND r.owner_user_id=$2 AND r.connector_id=$3").bind(workspace).bind(user).bind(connector).fetch_one(&mut conn).await?;
                let saved: serde_json::Value = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'state',state,'generation',generation::text,'completedVersion',completed_version::text,'progressVersion',progress_version::text,'committedPages',committed_pages,'reconciliationRequired',reconciliation_required,'retryAt',retry_at,'hasReadLease',sync_id IS NOT NULL) FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND id=$3")
                    .bind(workspace).bind(user).bind(connector).fetch_one(&mut conn).await?;
                // Count only. Sealed values/key identifiers never cross stdout.
                let credentials: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_credentials WHERE workspace_id=$1 AND owner_user_id=$2 AND connector_id=$3")
                    .bind(workspace).bind(user).bind(connector).fetch_one(&mut conn).await?;
                sqlx::query("ROLLBACK").execute(&mut conn).await?;
                conn.close().await?;
                output(serde_json::json!({"restrictedRole":true,"rows":rows,"connector":saved,"credentialRows":credentials})).await?;
            }
            Some("stop") => break,
            _ => return Err("invalid fixture command".into()),
        }
    }
    let _ = stop.send(());
    tokio::time::timeout(std::time::Duration::from_secs(5), server).await???;
    let clean = hub.shutdown().await.is_clean();
    if let Some(config) = meili.as_ref().filter(|_| index_cleanup_required) {
        let response = reqwest::Client::new()
            .delete(format!("{}/indexes/{}", config.url, config.index_uid))
            .bearer_auth(config.api_key())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err("owned fixture index cleanup failed".into());
        }
        let task: serde_json::Value = response.json().await?;
        fvoci_server::search::meili::wait_meili_tasks(
            config,
            &[task["taskUid"].as_u64().ok_or("cleanup task required")?],
        )
        .await?;
    }
    pool.close().await;
    upstream.shutdown().await;
    if borrowed.is_none() {
        tokio::fs::remove_dir_all(storage).await?;
    }
    if !clean {
        return Err("fixture native shutdown failed".into());
    }
    output(serde_json::json!({"stopped":true,"ownedResources":0})).await?;
    Ok(())
}
