use std::env;
use std::fmt;
use std::net::SocketAddr;

use crate::auth::password::Keyring;

pub struct Config {
    pub bind: SocketAddr,
    pub migration_url: String,
    pub app_database_url: String,
    pub password_keys: Keyring,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            bind: self.bind,
            migration_url: self.migration_url.clone(),
            app_database_url: self.app_database_url.clone(),
            password_keys: self.password_keys.clone(),
            branding_name: self.branding_name.clone(),
            public_origin: self.public_origin.clone(),
            cookie_secure: self.cookie_secure,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("migration_url", &"<redacted>")
            .field("app_database_url", &"<redacted>")
            .field("branding_name", &self.branding_name)
            .field("public_origin", &self.public_origin)
            .field("cookie_secure", &self.cookie_secure)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind = env::var("FVOCI_BIND")
            .unwrap_or_else(|_| "127.0.0.1:0".to_string())
            .parse()
            .map_err(|e| format!("invalid FVOCI_BIND: {e}"))?;

        let migration_url = env::var("DATABASE_URL")
            .or_else(|_| env::var("FVOCI_MIGRATION_URL"))
            .map_err(|_| "DATABASE_URL or FVOCI_MIGRATION_URL is required".to_string())?;

        let app_database_url = env::var("DATABASE_APP_URL")
            .or_else(|_| env::var("FVOCI_APP_DATABASE_URL"))
            .map_err(|_| {
                "DATABASE_APP_URL is required and must not fall back to migration URL".to_string()
            })?;

        if app_database_url == migration_url {
            return Err("DATABASE_APP_URL must differ from the migration owner URL".into());
        }

        let migration_role = database_role(&migration_url)?;
        let app_role = database_role(&app_database_url)?;
        if !migration_role.is_empty() && migration_role == app_role {
            return Err(
                "DATABASE_APP_URL must use a different database role than the migration owner"
                    .into(),
            );
        }

        let pepper_keys = env::var("PASSWORD_PEPPER_KEYS").map_err(|_| {
            "PASSWORD_PEPPER_KEYS is required (JSON map of key id to 64-char hex)".to_string()
        })?;
        let pepper_active = env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")
            .map_err(|_| "PASSWORD_PEPPER_ACTIVE_KEY_ID is required".to_string())?;
        let password_keys = Keyring::parse(&pepper_keys, &pepper_active)?;

        let branding_name = env::var("FVOCI_BRANDING_NAME").unwrap_or_else(|_| "FVOCI".to_string());
        let public_origin =
            env::var("FVOCI_PUBLIC_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".to_string());
        let cookie_secure = env::var("FVOCI_COOKIE_SECURE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(public_origin.starts_with("https://"));

        Ok(Self {
            bind,
            migration_url,
            app_database_url,
            password_keys,
            branding_name,
            public_origin,
            cookie_secure,
        })
    }
}

fn database_role(url: &str) -> Result<String, String> {
    url::Url::parse(url)
        .map(|parsed| parsed.username().to_string())
        .map_err(|e| format!("invalid database url: {e}"))
}
