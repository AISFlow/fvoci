use std::path::PathBuf;

use fvoci_server::db::migrate;
use fvoci_server::search::index::rebuild_search_index;
use fvoci_server::search::meili::{ensure_meili_key_file, meili_config_from_env};
use uuid::Uuid;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("fvoci-migrate: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            let url = migration_url()?;
            migrate::run_migrations(&url).await?;
        }
        [flag, role] if flag == "--grant-app-role" => {
            let url = migration_url()?;
            migrate::grant_app_role(&url, role).await?;
            eprintln!("granted app role privileges to {role}");
        }
        [flag, path] if flag == "--ensure-meili-key" => {
            ensure_meili_key_file(&PathBuf::from(path)).await?;
        }
        [flag] if flag == "--rebuild-search" => {
            rebuild(None).await?;
        }
        [flag, workspace] if flag == "--rebuild-search" => {
            let workspace_id = Uuid::parse_str(workspace).map_err(|_| "invalid workspace ID")?;
            rebuild(Some(workspace_id)).await?;
        }
        _ => {
            return Err(
                "usage: fvoci-migrate [--grant-app-role <role> | --ensure-meili-key <file> | --rebuild-search [workspace-id]]".into(),
            );
        }
    }
    Ok(())
}

async fn rebuild(workspace_id: Option<Uuid>) -> Result<(), Box<dyn std::error::Error>> {
    let url = migration_url()?;
    let meili =
        meili_config_from_env()?.ok_or("FVOCI_MEILI_URL is required for --rebuild-search")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    migrate::assert_schema_current(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let outcome = rebuild_search_index(&pool, &meili, workspace_id).await?;
    eprintln!(
        "search rebuild complete workspaces={} pages={}",
        outcome.workspaces, outcome.pages
    );
    pool.close().await;
    Ok(())
}

fn migration_url() -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_MIGRATION_URL"))
        .map_err(|_| "DATABASE_URL is required")?)
}
