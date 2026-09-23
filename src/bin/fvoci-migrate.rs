use fvoci_server::db::migrate;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_MIGRATION_URL"))
        .map_err(|_| "DATABASE_URL is required")?;
    migrate::run_migrations(&url).await?;
    Ok(())
}
