use std::path::PathBuf;

use fvoci_server::db::migrate;
use fvoci_server::search::meili::ensure_meili_key_file;

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
        _ => {
            return Err(
                "usage: fvoci-migrate [--grant-app-role <role> | --ensure-meili-key <file>]".into(),
            );
        }
    }
    Ok(())
}

fn migration_url() -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_MIGRATION_URL"))
        .map_err(|_| "DATABASE_URL is required")?)
}
