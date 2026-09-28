use std::path::PathBuf;

use fvoci_server::attachments::{verify_stored_objects, ObjectStorage};
use fvoci_server::auth::password::Keyring;
use fvoci_server::config::storage_settings_from_env;
use fvoci_server::db::migrate;
use fvoci_server::db::outbox_recover::{parse_recover_outbox_args, recover_outbox};
use fvoci_server::identity::encryption_keys_from_env;
use fvoci_server::search::index::{rebuild_pool, rebuild_search_index};
use fvoci_server::search::meili::{ensure_meili_key_file, meili_config_from_env};
use fvoci_server::secret_maintenance::{audit_secrets, rotate_secrets};
use fvoci_server::secret_verify::verify_sealed_secrets;
use uuid::Uuid;

fn main() {
    // Before the runtime starts any thread: `<VAR>_FILE` secrets become `<VAR>`.
    // In the standalone install's init container, also the owner URL and
    // Meilisearch master key from the mounted install files.
    if let Err(error) = fvoci_server::config::load_secret_files()
        .and_then(|()| fvoci_server::secret_bootstrap::load_install_env())
    {
        eprintln!("fvoci-migrate: {error}");
        std::process::exit(1);
    }
    async_main();
}

#[tokio::main]
async fn async_main() {
    if let Err(error) = run().await {
        eprintln!("fvoci-migrate: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, manifest, project, created, pg_version, dump, storage]
            if flag == "--backup-manifest" =>
        {
            fvoci_server::backup_manifest::create(
                &PathBuf::from(manifest),
                project,
                created,
                pg_version,
                &PathBuf::from(dump),
                &PathBuf::from(storage),
            )?;
        }
        [flag, manifest, dump, storage, project] if flag == "--restore-preflight" => {
            let (snapshot, since) = fvoci_server::backup_manifest::preflight(
                &PathBuf::from(manifest),
                &PathBuf::from(dump),
                &PathBuf::from(storage),
                project,
            )?;
            println!("{snapshot} {since}");
        }
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
        [flag] if flag == "--rotate-vapid" => {
            rotate_vapid().await?;
        }
        [flag] if flag == "--secrets-audit" => {
            secrets_audit().await?;
        }
        [flag] if flag == "--secrets-rotate" => {
            secrets_rotate().await?;
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
        [flag] if flag == "--install" => {
            fvoci_server::secret_bootstrap::install().await?;
        }
        [flag, rest @ ..] if flag == "--recover-outbox" => {
            let url = migration_url()?;
            let opts = parse_recover_outbox_args(rest)?;
            let report = recover_outbox(&url, opts).await?;
            println!("{}", serde_json::to_string(&report)?);
        }
        _ => {
            return Err(
                "usage: fvoci-migrate [--grant-app-role <role> | --ensure-meili-key <file> | --rebuild-search [workspace-id] | --verify-storage | --verify-secrets | --rotate-vapid | --secrets-audit | --secrets-rotate | --doctor | --init-env --public-origin <url> --out <path> [--yes] | --install | --recover-outbox --since <utc> --snapshot-at <utc> [--apply --reason <text> --ack-external-replay] | --backup-manifest <manifest> <project> <created-utc> <pg-version> <dump> <storage-tar> | --restore-preflight <manifest> <dump> <storage-tar> <target-project>]".into(),
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
/// workspace SSO, webhooks, VAPID) opens with the configured keyring. Runs with the
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

/// Source `fvoci secrets rotate-vapid`: new Web Push keypair, every browser
/// subscription revoked, `instance.vapid_rotated` event + audit in one system
/// transaction. Runs as the app role like `--verify-secrets`. Prints the new
/// public key and the revoked count, never the private key.
async fn rotate_vapid() -> Result<(), Box<dyn std::error::Error>> {
    let url = app_url("--rotate-vapid")?;
    let keys =
        encryption_keys_from_env()?.ok_or("ENCRYPTION_KEYS is required for --rotate-vapid")?;
    let pool = fvoci_server::db::pool::connect_app(&url).await?;
    let outcome = fvoci_server::push::rotate_vapid_keys(&pool, &keys).await?;
    pool.close().await;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "publicKey": outcome.public_key,
            "revokedSubscriptions": outcome.revoked_subscriptions,
        }))?
    );
    Ok(())
}

/// Source `fvoci secrets audit`: key ids in use per secret class, password
/// pepper key ids, values not under the active key and missing key ids. Prints
/// the report (counts and key ids only) and exits 1 when `problems` is nonzero.
async fn secrets_audit() -> Result<(), Box<dyn std::error::Error>> {
    let url = app_url("--secrets-audit")?;
    let keys =
        encryption_keys_from_env()?.ok_or("ENCRYPTION_KEYS is required for --secrets-audit")?;
    let peppers = password_keys_from_env()?;
    let pool = maintenance_pool(&url).await?;
    let report = audit_secrets(&pool, &keys, &peppers).await;
    pool.close().await;
    let report = report?;
    println!("{}", serde_json::to_string(&report)?);
    if report.problems > 0 {
        return Err(format!(
            "{} secret problem(s); missing key ids {:?}, password key ids {:?}",
            report.problems, report.missing_key_ids, report.missing_password_key_ids
        )
        .into());
    }
    Ok(())
}

/// Source `fvoci secrets rotate`: re-seals every value under
/// `ENCRYPTION_ACTIVE_KEY_ID`. Refuses before any write if a value does not
/// open. Safe to re-run; prints `{"changed":n,"unchanged":m}`.
async fn secrets_rotate() -> Result<(), Box<dyn std::error::Error>> {
    let url = app_url("--secrets-rotate")?;
    let keys =
        encryption_keys_from_env()?.ok_or("ENCRYPTION_KEYS is required for --secrets-rotate")?;
    let pool = maintenance_pool(&url).await?;
    let report = rotate_secrets(&pool, &keys).await;
    pool.close().await;
    println!("{}", serde_json::to_string(&report?)?);
    Ok(())
}

/// Source `assertRuntimeDatabase`: the app role (not a superuser, BYPASSRLS
/// or schema owner) on the schema this binary was built for.
async fn maintenance_pool(url: &str) -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
    let pool = fvoci_server::db::pool::connect_app_with_max(url, 1).await?;
    let checked = async {
        migrate::assert_app_role(&pool).await?;
        migrate::assert_schema_current(&pool).await
    }
    .await;
    if let Err(error) = checked {
        pool.close().await;
        return Err(error.into());
    }
    Ok(pool)
}

fn password_keys_from_env() -> Result<Keyring, Box<dyn std::error::Error>> {
    let keys = std::env::var("PASSWORD_PEPPER_KEYS")
        .map_err(|_| "PASSWORD_PEPPER_KEYS is required for --secrets-audit")?;
    let active = std::env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")
        .map_err(|_| "PASSWORD_PEPPER_ACTIVE_KEY_ID is required for --secrets-audit")?;
    Ok(Keyring::parse(&keys, &active)?)
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
