use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect_app(url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new().max_connections(10).connect(url).await
}
