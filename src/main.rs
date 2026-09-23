use std::net::SocketAddr;
use std::sync::Arc;

use tokio::signal;
use tracing_subscriber::EnvFilter;

use fvoci_server::auth::AuthService;
use fvoci_server::collab::{CollabConfig, CollabHub};
use fvoci_server::config::Config;
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router, state::AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("fvoci_server=info".parse()?))
        .init();

    let config = Config::from_env()?;
    migrate::run_migrations(&config.migration_url).await?;

    let pool = pool::connect_app(&config.app_database_url).await?;
    let run_result = run_server(config, pool.clone()).await;
    pool.close().await;
    run_result?;
    Ok(())
}

async fn run_server(config: Config, pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    migrate::assert_app_role(&pool).await?;

    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let addr = listener.local_addr()?;
    let public_origin =
        fvoci_server::http::guard::resolve_public_origin(&config.public_origin, addr)?;
    eprintln!("fvoci-server listening on http://{addr}");

    let collab = CollabConfig::from_env().map(|cfg| Arc::new(CollabHub::new(cfg, pool.clone())));
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: config.password_keys.clone(),
        }),
        branding_name: config.branding_name.clone(),
        public_origin,
        cookie_secure: config.cookie_secure,
        rate_limiter: RateLimiter::new(),
        collab: collab.clone(),
    };

    let serve_result = axum::serve(
        listener,
        router(state, config.static_dir.clone())
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;

    // Upgraded WebSockets outlive the HTTP graceful-shutdown watcher. Join the
    // room owners and reap their native helpers before dropping the DB pool.
    // Also clean up if serving fails; do not return early on that error.
    if let Some(hub) = collab {
        hub.shutdown().await;
    }
    serve_result?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if signal::ctrl_c().await.is_ok() {
            eprintln!("shutdown signal received (Ctrl+C)");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut stream) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            if stream.recv().await.is_some() {
                eprintln!("shutdown signal received (SIGTERM)");
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
