use std::path::PathBuf;

use fvoci_server::attachments::{verify_stored_objects, ObjectStorage};
use fvoci_server::config::storage_settings_from_env;
use fvoci_server::db::migrate;
use fvoci_server::db::outbox_recover::{parse_recover_outbox_args, recover_outbox};
use fvoci_server::identity::encryption_keys_from_env;
use fvoci_server::search::index::{rebuild_pool, rebuild_search_index};
use fvoci_server::search::meili::{ensure_meili_key_file, meili_config_from_env};
use fvoci_server::secret_verify::verify_sealed_secrets;
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
        [flag] if flag == "--verify-storage" => {
            verify_storage().await?;
        }
        [flag] if flag == "--verify-secrets" => {
            verify_secrets().await?;
        }
        [flag] if flag == "--doctor" => {
            let report = fvoci_server::doctor::run_doctor().await;
            println!("{}", serde_json::to_string(&report)?);
            if !report.ok {
                std::process::exit(1);
            }
        }
        [flag, rest @ ..] if flag == "--init-env" => {
            let flags = fvoci_server::init_env::parse_init_env_flags(rest)?;
            let path = fvoci_server::init_env::write_env(&flags)?;
            println!("{}", path.display());
        }
        [flag, rest @ ..] if flag == "--recover-outbox" => {
            let url = migration_url()?;
            let opts = parse_recover_outbox_args(rest)?;
            let report = recover_outbox(&url, opts).await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        _ => {
            return Err(
                "usage: fvoci-migrate [--grant-app-role <role> | --ensure-meili-key <file> | --rebuild-search [workspace-id] | --verify-storage | --verify-secrets | --doctor | --init-env --public-origin <url> --out <path> [--yes] | --recover-outbox --since <utc> --snapshot-at <utc> [--apply --reason <text> --ack-external-replay]]".into(),
            );
        }
    }
    Ok(())
}

async fn rebuild(workspace_id: Option<Uuid>) -> Result<(), Box<dyn std::error::Error>> {
    let url = migration_url()?;
    let meili =
        meili_config_from_env()?.ok_or("FVOCI_MEILI_URL is required for --rebuild-search")?;
    let pool = rebuild_pool(&url).await?;
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

/// Post-restore check: every stored attachment and its published preview
/// exist in the configured storage (local volume or S3 bucket) with their
/// recorded sizes, and every
/// branding asset the instance settings reference exists with its digest. Runs with the
/// server's environment (`DATABASE_APP_URL` and the storage variables), so it
/// needs no owner credentials and reads under the app role's RLS.
async fn verify_storage() -> Result<(), Box<dyn std::error::Error>> {
    let url = app_url("--verify-storage")?;
    let storage = ObjectStorage::from_settings(&storage_settings_from_env()?)?;
    storage.probe().await?;
    let pool = fvoci_server::db::pool::connect_app(&url).await?;
    let report = verify_stored_objects(&pool, &storage).await;
    pool.close().await;
    let report = report?;
    println!("{}", serde_json::to_string(&report)?);
    if !report.is_complete() {
        return Err(format!(
            "storage is missing {} and has {} size-mismatched stored attachment(s); previews missing {}, size-mismatched {}; branding assets missing {:?}, mismatched {:?}",
            report.missing.len(),
            report.size_mismatch.len(),
            report.preview_missing.len(),
            report.preview_size_mismatch.len(),
            report.branding_missing,
            report.branding_mismatch
        )
        .into());
    }
    Ok(())
}

/// Post-restore check: every secret sealed with `ENCRYPTION_KEYS` (MFA,
/// workspace SSO, webhooks) opens with the configured keyring. Runs with the
/// server's environment (`DATABASE_APP_URL`, `ENCRYPTION_KEYS`); the app role
/// reads the ciphertext columns in the system context, like the server.
/// Prints counts and failing row ids, never secret values.
async fn verify_secrets() -> Result<(), Box<dyn std::error::Error>> {
    let url = app_url("--verify-secrets")?;
    let keys = encryption_keys_from_env()?;
    let pool = fvoci_server::db::pool::connect_app(&url).await?;
    let report = verify_sealed_secrets(&pool, keys.as_deref()).await;
    pool.close().await;
    let report = report?;
    println!("{}", serde_json::to_string(&report)?);
    if !report.is_complete() {
        return Err(format!(
            "{} sealed secret(s) do not open with the configured ENCRYPTION_KEYS (key ids in use: {:?})",
            report.failed(),
            report.key_ids_in_use.keys().collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}

fn app_url(flag: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::env::var("DATABASE_APP_URL")
        .or_else(|_| std::env::var("FVOCI_APP_DATABASE_URL"))
        .map_err(|_| format!("DATABASE_APP_URL is required for {flag}"))?)
}

fn migration_url() -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_MIGRATION_URL"))
        .map_err(|_| "DATABASE_URL is required")?)
}
