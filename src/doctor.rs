//! `fvoci-migrate --doctor` (source `apps/server/src/doctor.ts`): checks the
//! server's own environment and dependencies without starting it and prints
//! `{"ok":bool,"checks":[{"name","ok","detail"?}]}`. Run it with the server's
//! environment (it needs no owner credentials). Checks are read-only: nothing
//! is migrated or sent, and nothing is left behind (the local storage check
//! writes and removes one probe file). Details never carry secrets; database
//! URLs in driver messages are masked.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::attachments::ObjectStorage;
use crate::auth::password::Keyring;
use crate::collab::config::CollabConfig;
use crate::config::Config;
use crate::db::{migrate, pool};

const DB_TIMEOUT: Duration = Duration::from_secs(10);
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

struct Checks(Vec<DoctorCheck>);

impl Checks {
    fn pass(&mut self, name: &'static str, detail: Option<String>) {
        self.0.push(DoctorCheck {
            name,
            ok: true,
            detail,
        });
    }

    fn fail(&mut self, name: &'static str, detail: impl Into<String>) {
        self.0.push(DoctorCheck {
            name,
            ok: false,
            detail: Some(mask_urls(&detail.into())),
        });
    }

    fn result(&mut self, name: &'static str, result: Result<Option<String>, String>) {
        match result {
            Ok(detail) => self.pass(name, detail),
            Err(detail) => self.fail(name, detail),
        }
    }
}

/// `scheme://user:password@` → `scheme://***@` in any message.
pub fn mask_urls(text: &str) -> String {
    // Greedy up to the last `@` of the token, so a userinfo containing a raw
    // `/` or `@` is masked too (over-masking a path is harmless).
    let re = regex::Regex::new(r"([A-Za-z][A-Za-z0-9+.-]*://)\S*@").expect("static pattern");
    re.replace_all(text, "${1}***@").into_owned()
}

/// Keys published in development fixtures (source dev.env); compromised by
/// publication, not by entropy.
fn has_public_dev_key(ring: &Keyring) -> bool {
    ring.keys
        .values()
        .any(|k| k.len() == 32 && (k.iter().all(|b| *b == 0xaa) || k.iter().all(|b| *b == 0xee)))
}

fn disabled(what: &str) -> Result<Option<String>, String> {
    Ok(Some(format!("disabled ({what} unset)")))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub async fn run_doctor() -> DoctorReport {
    let mut checks = Checks(Vec::new());
    // What the server itself does at startup; one bad variable fails it. The
    // checks below parse each setting on its own so every problem is named.
    checks.result("env", Config::from_env().map(|_| None));

    checks.result(
        "password_pepper_keys",
        match (
            env_value("PASSWORD_PEPPER_KEYS"),
            env_value("PASSWORD_PEPPER_ACTIVE_KEY_ID"),
        ) {
            (Some(keys), Some(active)) => match Keyring::parse(&keys, &active) {
                Ok(ring) if has_public_dev_key(&ring) => {
                    Err("public development key; replace before deployment".into())
                }
                Ok(_) => Ok(None),
                Err(err) => Err(err),
            },
            _ => Err("PASSWORD_PEPPER_KEYS and PASSWORD_PEPPER_ACTIVE_KEY_ID are required".into()),
        },
    );
    let origin = env_value("FVOCI_PUBLIC_ORIGIN")
        .ok_or_else(|| {
            "FVOCI_PUBLIC_ORIGIN is unset (server default http://localhost:5173)".to_string()
        })
        .and_then(|raw| crate::http::guard::normalize_public_origin(&raw));
    checks.result(
        "public_origin",
        origin.clone().and_then(|origin| {
            let cookie_secure = env_value("FVOCI_COOKIE_SECURE")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(origin.starts_with("https://"));
            public_origin_check(&origin, cookie_secure)
        }),
    );
    checks.result(
        "encryption_keys",
        match crate::identity::encryption_keys_from_env() {
            Ok(None) => disabled("ENCRYPTION_KEYS"),
            Ok(Some(ring)) if has_public_dev_key(&ring) => {
                Err("public development key; replace before deployment".into())
            }
            Ok(Some(_)) => Ok(None),
            Err(err) => Err(err),
        },
    );
    if let Ok(origin) = &origin {
        checks.result(
            "identity",
            crate::identity::Identity::from_env(origin).map(|_| None),
        );
    }
    checks.result(
        "integrations",
        crate::integrations::Integrations::from_env().map(|_| None),
    );

    let collab = collab_settings(&mut checks);
    match env_value("DATABASE_APP_URL").or_else(|| env_value("FVOCI_APP_DATABASE_URL")) {
        Some(url) => database_checks(&mut checks, &url, collab.as_ref()).await,
        None => checks.fail("database", "DATABASE_APP_URL is required"),
    }
    checks.result("storage", storage_check().await);
    checks.result(
        "meilisearch",
        match crate::search::meili::meili_config_from_env() {
            Ok(None) => disabled("FVOCI_MEILI_URL"),
            Ok(Some(meili)) => crate::search::meili::probe_meili_index(&meili)
                .await
                .map(|()| None)
                .map_err(|err| format!("search service probe failed: {err}")),
            Err(err) => Err(err),
        },
    );
    checks.result(
        "smtp",
        match crate::mail::smtp_from_env() {
            Ok(None) => disabled("SMTP_HOST/SMTP_PORT/SMTP_FROM"),
            Ok(Some(smtp)) => crate::mail::probe_smtp(&smtp)
                .await
                .map(|()| None)
                .map_err(|code| format!("{code} ({}:{})", smtp.host, smtp.port)),
            Err(err) => Err(err),
        },
    );
    if let Some(collab) = &collab {
        checks.result("collab_engine", collab_engine_check(collab).await);
    }
    checks.result(
        "extractor",
        match env_value("FVOCI_EXTRACTOR_BIN") {
            Some(v) => {
                crate::attachments::validate_extractor_bin(&PathBuf::from(v.trim())).map(|()| None)
            }
            None => disabled("FVOCI_EXTRACTOR_BIN"),
        },
    );

    let ok = checks.0.iter().all(|c| c.ok);
    DoctorReport {
        ok,
        checks: checks.0,
    }
}

fn public_origin_check(origin: &str, cookie_secure: bool) -> Result<Option<String>, String> {
    let url = url::Url::parse(origin).map_err(|e| e.to_string())?;
    if url.scheme() == "https" {
        return if cookie_secure {
            Ok(None)
        } else {
            Err("https origin with FVOCI_COOKIE_SECURE=false; set it to true".into())
        };
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if loopback {
        Ok(Some("http loopback origin (local evaluation only)".into()))
    } else {
        Err(format!(
            "{origin} is plain http; terminate TLS in a reverse proxy and use https"
        ))
    }
}

/// `FVOCI_COLLAB_ENGINE` set but unusable would silently disable collab at
/// startup; report it instead.
fn collab_settings(checks: &mut Checks) -> Option<CollabConfig> {
    let raw = std::env::var("FVOCI_COLLAB_ENGINE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    match CollabConfig::from_env() {
        Some(cfg) => Some(cfg),
        None => {
            checks.fail(
                "collab_engine",
                format!(
                    "FVOCI_COLLAB_ENGINE is not a file ({}); collaboration would be disabled",
                    raw.trim()
                ),
            );
            None
        }
    }
}

async fn database_checks(checks: &mut Checks, url: &str, collab: Option<&CollabConfig>) {
    let connected = tokio::time::timeout(DB_TIMEOUT, pool::connect_app_with_max(url, 1)).await;
    let pool = match connected {
        Ok(Ok(pool)) => {
            checks.pass("database", None);
            pool
        }
        Ok(Err(err)) => {
            checks.fail("database", format!("connect failed: {err}"));
            return;
        }
        Err(_) => {
            checks.fail("database", "connect timed out");
            return;
        }
    };
    checks.result(
        "app_role",
        migrate::assert_app_role(&pool).await.map(|()| None),
    );
    checks.result(
        "schema_version",
        migrate::assert_schema_current(&pool)
            .await
            .map(|()| None)
            .map_err(|err| format!("{err}; run fvoci-migrate then --grant-app-role")),
    );
    let max_rooms = collab.map_or(0, |c| c.max_rooms);
    checks.result(
        "pg_connection_budget",
        crate::collab::config::assert_collab_fits_postgres(&pool, max_rooms)
            .await
            .map(|()| {
                Some(format!(
                    "needs {} (collab rooms {max_rooms})",
                    crate::collab::config::collab_pg_connections_required(max_rooms)
                ))
            }),
    );
    pool.close().await;
}

async fn storage_check() -> Result<Option<String>, String> {
    let driver = std::env::var("STORAGE_DRIVER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "local".to_string());
    if driver == "local" {
        // Inspect only: the server creates the root at start, the doctor must
        // not (it may run as another user or on another host).
        let root = crate::config::storage_root_path_from_values(
            std::env::var("FVOCI_STORAGE_DIR").ok().as_deref(),
            std::env::var("STORAGE_LOCAL_PATH").ok().as_deref(),
        )?;
        let meta = std::fs::metadata(&root)
            .map_err(|e| format!("storage root {}: {e}", root.display()))?;
        if !meta.is_dir() {
            return Err(format!(
                "storage root {} is not a directory",
                root.display()
            ));
        }
        // Writability: a probe file is created and removed again.
        let probe = root.join(format!(".fvoci-doctor-{}", uuid::Uuid::now_v7().simple()));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|e| format!("storage root {} is not writable: {e}", root.display()))?;
        let _ = std::fs::remove_file(&probe);
        return Ok(Some("local".to_string()));
    }
    let settings = crate::config::storage_settings_from_env()?;
    let storage = ObjectStorage::from_settings(&settings)?;
    storage.probe().await?;
    Ok(Some(
        match settings {
            crate::config::StorageSettings::Local { .. } => "local",
            crate::config::StorageSettings::S3(_) => "s3",
        }
        .to_string(),
    ))
}

async fn collab_engine_check(collab: &CollabConfig) -> Result<Option<String>, String> {
    use collab_engine::outcome::EngineStatus;
    let bridge = crate::collab::engine_bridge::EngineBridge::spawn(
        collab.engine_bin.clone(),
        collab_engine::limits::Limits::default(),
    )
    .map_err(|report| format!("spawn failed: {:?}", report.outcome))?;
    let reply = tokio::time::timeout(
        HELPER_TIMEOUT,
        bridge.call(collab_engine::protocol::Request::Ping),
    )
    .await;
    let _ = bridge.stop().await;
    match reply {
        Ok(Ok(report)) => match report.outcome {
            EngineStatus::Ok { .. } => Ok(None),
            other => Err(format!("ping failed: {other:?}")),
        },
        Ok(Err(_)) => Err("engine bridge stopped".into()),
        Err(_) => Err("ping timed out".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_credentials_in_urls() {
        assert_eq!(
            mask_urls("connect postgres://app:s3cret@db:5432/fvoci failed"),
            "connect postgres://***@db:5432/fvoci failed"
        );
        assert_eq!(mask_urls("postgres://u:p@ss@db/x"), "postgres://***@db/x");
        assert_eq!(mask_urls("no url here"), "no url here");
    }

    #[test]
    fn public_dev_keys_are_flagged() {
        let dev = Keyring::parse(
            r#"{"k":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            "k",
        )
        .unwrap();
        assert!(has_public_dev_key(&dev));
        let real = Keyring::parse(
            r#"{"k":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#,
            "k",
        )
        .unwrap();
        assert!(!has_public_dev_key(&real));
    }
}

#[cfg(test)]
mod review_tests {
    use super::mask_urls;

    #[test]
    fn masks_userinfo_containing_a_slash() {
        assert_eq!(
            mask_urls("connect postgres://u:pa/ss@db:5432/x failed"),
            "connect postgres://***@db:5432/x failed"
        );
        assert_eq!(mask_urls("postgres://u:p@ss@db/x"), "postgres://***@db/x");
        assert_eq!(mask_urls("no url here"), "no url here");
    }
}
