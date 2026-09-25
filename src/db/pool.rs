use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect_app(url: &str) -> Result<PgPool, sqlx::Error> {
    connect_app_with_max(url, crate::collab::config::APP_POOL_MAX_CONNECTIONS).await
}

pub async fn connect_app_with_max(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .connect(url)
        .await
}
