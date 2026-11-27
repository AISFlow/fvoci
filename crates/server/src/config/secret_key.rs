//! The key the server seals secrets with: from the environment only.

use anyhow::bail;
use serde_json::{Map, Value};

use crate::auth::sealed::SecretKey;

/// The environment variable of the key the server seals secrets with: the one way the
/// key is given.
pub const SECRET_KEY_ENV: &str = "FVOCI_SECRET_KEY";

/// The settings that are read from the environment only, each with its variable: what
/// `docs/configuration.md` lists under "그 밖의 환경 변수" besides the others.
pub const ENV_ONLY: [(&str, &str); 1] = [("secret_key", SECRET_KEY_ENV)];

/// The key, from [`SECRET_KEY_ENV`]: none if the variable is unset or blank. The file
/// does not give it, whatever `is_unset` says of a value it has there: a key in a file
/// is one more copy of it that is read by whoever reads the file.
pub(super) fn read(
    keys: &Map<String, Value>,
    env: &impl Fn(&str) -> Option<String>,
    is_unset: fn(&Value) -> bool,
) -> anyhow::Result<Option<SecretKey>> {
    for (key, variable) in ENV_ONLY {
        if keys.get(key).is_some_and(|value| !is_unset(value)) {
            bail!("{key} is not read from the configuration file: give it as {variable}");
        }
    }
    match env(SECRET_KEY_ENV) {
        Some(text) if !text.trim().is_empty() => SecretKey::from_base64(&text)
            .map(Some)
            .map_err(|error| anyhow::anyhow!("{SECRET_KEY_ENV}: {error}")),
        _ => Ok(None),
    }
}
