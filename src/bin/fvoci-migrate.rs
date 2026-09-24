use fvoci_server::db::migrate;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("fvoci-migrate: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_MIGRATION_URL"))
        .map_err(|_| "DATABASE_URL is required")?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => migrate::run_migrations(&url).await?,
        [flag, role] if flag == "--grant-app-role" => {
            migrate::grant_app_role(&url, role).await?;
            eprintln!("granted app role privileges to {role}");
        }
        _ => return Err("usage: fvoci-migrate [--grant-app-role <role>]".into()),
    }
    Ok(())
}
