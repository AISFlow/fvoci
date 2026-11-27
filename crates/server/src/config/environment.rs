//! The configuration keys that environment variables can override.

use anyhow::bail;
use serde_json::Value;

/// How the text of an environment variable becomes a configuration value.
#[derive(Debug, Clone, Copy)]
pub(super) enum EnvKind {
    Text,
    Number,
    Flag,
    /// A comma-separated list.
    List,
}

/// The configuration keys that an environment variable can override, a key of
/// `file_storage_s3` as `file_storage_s3.<key>`. The variable is [`super::env_name`] of
/// the key, for example `FVOCI_SESSION_IDLE_DAYS` and `FVOCI_FILE_STORAGE_S3_BUCKET`. The
/// S3 credentials have none: the S3 client reads them from the `AWS_*` variables when
/// `access_key_id` is empty.
pub(super) const ENV_KEYS: [(&str, EnvKind); 62] = [
    ("server_url", EnvKind::Text),
    ("port", EnvKind::Number),
    ("database_kind", EnvKind::Text),
    ("database_connection", EnvKind::Text),
    ("database_max_connections", EnvKind::Number),
    ("database_connect_attempts", EnvKind::Number),
    ("server_lock_wait_seconds", EnvKind::Number),
    ("serve_https", EnvKind::Flag),
    ("web_app_path", EnvKind::Text),
    ("file_storage", EnvKind::Text),
    ("file_storage_path", EnvKind::Text),
    ("file_storage_s3.bucket", EnvKind::Text),
    ("file_storage_s3.key_prefix", EnvKind::Text),
    ("file_storage_s3.region", EnvKind::Text),
    ("file_storage_s3.endpoint", EnvKind::Text),
    ("file_storage_s3.https", EnvKind::Flag),
    ("file_storage_s3.server_side_encryption", EnvKind::Flag),
    ("file_storage_s3.timeout_ms", EnvKind::Number),
    ("max_file_bytes", EnvKind::Number),
    ("max_org_storage_bytes", EnvKind::Number),
    ("metrics_address", EnvKind::Text),
    ("metrics_token", EnvKind::Text),
    ("block_change_webhooks", EnvKind::List),
    ("allowed_origins", EnvKind::List),
    ("session_idle_days", EnvKind::Number),
    ("session_max_days", EnvKind::Number),
    ("listen_loopback_only", EnvKind::Flag),
    ("serve_admin_socket", EnvKind::Flag),
    ("admin_socket_path", EnvKind::Text),
    ("allow_public_links", EnvKind::Flag),
    ("name_display", EnvKind::Text),
    ("show_email_addresses", EnvKind::Flag),
    ("oidc_allow_private_addresses", EnvKind::Flag),
    ("oidc_extra_ca_file", EnvKind::Text),
    ("oidc_former_provider_days", EnvKind::Number),
    ("org_creation", EnvKind::Text),
    ("websocket_ping_seconds", EnvKind::Number),
    ("public_link_connections_per_container", EnvKind::Number),
    ("public_link_connections_per_address", EnvKind::Number),
    ("compress_responses", EnvKind::Flag),
    ("security_contact", EnvKind::Text),
    ("max_request_bytes", EnvKind::Number),
    ("trash_retention_days", EnvKind::Number),
    ("upload_part_bytes", EnvKind::Number),
    ("tmp_path", EnvKind::Text),
    ("max_pending_upload_bytes", EnvKind::Number),
    ("max_import_bytes", EnvKind::Number),
    ("max_comment_versions", EnvKind::Number),
    ("max_file_versions", EnvKind::Number),
    ("max_concurrent_exports", EnvKind::Number),
    ("export_stall_seconds", EnvKind::Number),
    ("max_concurrent_imports", EnvKind::Number),
    ("trusted_proxies", EnvKind::List),
    ("login_pair_free_failures", EnvKind::Number),
    ("login_pair_base_delay_seconds", EnvKind::Number),
    ("login_pair_max_delay_seconds", EnvKind::Number),
    ("login_account_max_failures", EnvKind::Number),
    ("login_account_lock_seconds", EnvKind::Number),
    ("login_address_attempts_per_minute", EnvKind::Number),
    ("password_hash_concurrency", EnvKind::Number),
    ("password_hash_queue", EnvKind::Number),
    ("password_hash_queue_seconds", EnvKind::Number),
];

/// The value the environment's `text` for the variable `name` gives a key of `kind`.
///
/// # Errors
///
/// Returns an error naming the variable if the text is no value of that kind.
pub(super) fn env_value(name: &str, text: &str, kind: EnvKind) -> anyhow::Result<Value> {
    Ok(match kind {
        EnvKind::Text => Value::from(text),
        EnvKind::Number => {
            let trimmed = text.trim();
            if let Ok(number) = trimmed.parse::<i64>() {
                Value::from(number)
            } else if let Ok(number) = trimmed.parse::<u64>() {
                Value::from(number)
            } else {
                bail!("{name}={text:?} is not a whole number");
            }
        }
        EnvKind::Flag => match text.trim().to_lowercase().as_str() {
            "true" | "1" | "t" | "yes" | "on" => Value::Bool(true),
            "false" | "0" | "f" | "no" | "off" => Value::Bool(false),
            _ => bail!("{name}={text:?} is not true or false"),
        },
        EnvKind::List => Value::from(
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .collect::<Vec<&str>>(),
        ),
    })
}
