//! Instance settings store (source `packages/core/src/settings.ts`).
//!
//! The source keeps a per-process cache invalidated over Redis. This server
//! has no Redis, so every read resolves the (tiny) `instance_settings` table:
//! a write is visible to the next request in every process. The first
//! resolution in a process is kept as the boot snapshot, which answers "which
//! restart-required keys changed since this process started".

pub mod catalog;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde_json::{json, Map, Value};
use sqlx::PgPool;
use uuid::Uuid;

use catalog::Safety;
pub use catalog::{
    BrandingAsset, BrandingAssetKind, BrandingSettings, DefaultsUserSettings, SettingsKey,
    SettingsValues, SharePolicy, BRANDING_ASSET_MIME, SETTINGS_KEYS,
};

use crate::db::admin::{record_instance_change, require_live_instance_admin, InstanceChange};
use crate::db::context::set_system;

/// Enterprise features the admin form may unlock. The source reads them from a
/// signed license (`packages/ee`); license verification is not ported, so the
/// features this server implements are reported as enabled.
pub const EE_FEATURES_ENABLED: [&str; 2] = ["audit", "branding"];

/// Process-local boot snapshot shared by every clone of one `Db`.
#[derive(Clone, Default)]
pub struct SettingsBoot(Arc<OnceLock<SettingsValues>>);

#[derive(Debug, Clone)]
pub struct SettingsSnapshot {
    pub revision: i64,
    pub values: SettingsValues,
    /// Raw stored rows, valid or not (the write baseline).
    pub stored: BTreeMap<&'static str, Value>,
    /// Keys whose stored row is valid and therefore applied.
    pub overridden: Vec<SettingsKey>,
    /// `key.leaf` paths where an environment variable won.
    pub env_applied: Vec<String>,
}

impl SettingsSnapshot {
    pub fn restart_pending(&self, boot: &SettingsValues) -> Vec<SettingsKey> {
        SETTINGS_KEYS
            .into_iter()
            .filter(|key| {
                key.safety() == Safety::RestartRequired
                    && self.values.get_json(*key) != boot.get_json(*key)
            })
            .collect()
    }
}

pub fn asset_href(kind: BrandingAssetKind, asset: Option<&BrandingAsset>) -> Option<String> {
    asset.map(|asset| {
        format!(
            "/api/v1/branding/{}?v={}",
            kind.as_str(),
            &asset.sha256[..12.min(asset.sha256.len())]
        )
    })
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn resolve(rows: Vec<(String, Value)>, revision: i64, brand_default: &str) -> SettingsSnapshot {
    let mut values = SettingsValues::defaults(brand_default);
    let mut stored = BTreeMap::new();
    let mut overridden = Vec::new();
    let mut env_applied = Vec::new();
    let by_key: BTreeMap<String, Value> = rows.into_iter().collect();
    for key in SETTINGS_KEYS {
        if let Some(raw) = by_key.get(key.as_str()) {
            stored.insert(key.as_str(), raw.clone());
            if values.set_json(key, raw).is_some() {
                overridden.push(key);
            } else {
                // A broken row must not keep the instance from starting.
                tracing::warn!(
                    key = key.as_str(),
                    reason = "schema",
                    "settings.row_invalid"
                );
            }
        }
        let fallbacks = key.env_fallback();
        if fallbacks.is_empty() {
            continue;
        }
        let mut merged = values.get_json(key);
        let mut hits = Vec::new();
        for (leaf, name) in fallbacks {
            let Some(raw) = env_value(name) else { continue };
            let coerced = match merged.get(*leaf) {
                Some(Value::Bool(_)) => Value::Bool(raw == "1"),
                Some(Value::Number(_)) => match raw.parse::<i64>() {
                    Ok(n) => json!(n),
                    Err(_) => {
                        tracing::warn!(key = key.as_str(), reason = "env", "settings.row_invalid");
                        continue;
                    }
                },
                _ => Value::String(raw),
            };
            merged[*leaf] = coerced;
            hits.push(format!("{}.{}", key.as_str(), leaf));
        }
        if hits.is_empty() {
            continue;
        }
        if values.set_json(key, &merged).is_some() {
            env_applied.extend(hits);
        } else {
            tracing::warn!(key = key.as_str(), reason = "env", "settings.row_invalid");
        }
    }
    SettingsSnapshot {
        revision,
        values,
        stored,
        overridden,
        env_applied,
    }
}

async fn load_rows<'e, E>(executor: E) -> Result<Vec<(String, Value)>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_as::<_, (String, Value)>("SELECT key, value FROM fvoci.instance_settings")
        .fetch_all(executor)
        .await
}

async fn load_revision<'e, E>(executor: E) -> Result<i64, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT revision FROM fvoci.instance_settings_meta WHERE id = 1")
        .fetch_one(executor)
        .await
}

/// Current settings; records the boot snapshot on the first call.
pub async fn load(
    pool: &PgPool,
    boot: &SettingsBoot,
    brand_default: &str,
) -> Result<SettingsSnapshot, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let revision = load_revision(&mut *tx).await?;
    let rows = load_rows(&mut *tx).await?;
    tx.commit().await?;
    let snapshot = resolve(rows, revision, brand_default);
    let _ = boot.0.get_or_init(|| snapshot.values.clone());
    Ok(snapshot)
}

pub fn boot_values<'a>(boot: &'a SettingsBoot, current: &'a SettingsValues) -> &'a SettingsValues {
    boot.0.get().unwrap_or(current)
}

/// Read API for other features: the instance share-link policy.
pub async fn share_policy(pool: &PgPool) -> Result<SharePolicy, sqlx::Error> {
    let rows = load_rows(pool).await?;
    Ok(resolve(rows, 0, "FVOCI").values.share)
}

/// Effective values without touching the boot snapshot.
pub async fn current_values(
    pool: &PgPool,
    brand_default: &str,
) -> Result<SettingsValues, sqlx::Error> {
    let rows = load_rows(pool).await?;
    Ok(resolve(rows, 0, brand_default).values)
}

pub enum SettingsChange {
    /// Validated PATCH values: `None` resets the key (deletes its row).
    /// A `branding` value carries no asset leaves; the current ones are kept.
    Patch(Vec<(SettingsKey, Option<Value>)>),
    /// Upload route: set or clear one branding asset.
    BrandingAsset {
        kind: BrandingAssetKind,
        asset: Option<BrandingAsset>,
    },
}

#[derive(Debug)]
pub enum SettingsWriteError {
    /// The actor is not (or no longer) a live instance admin.
    NotAdmin,
    /// Removing an asset that is not set.
    AssetMissing,
}

pub struct SettingsWriteOutcome {
    pub changed: Vec<SettingsKey>,
    /// The asset the change replaced or removed; its object may be deleted
    /// after commit.
    pub previous_asset: Option<BrandingAsset>,
    pub snapshot: SettingsSnapshot,
}

fn diff_paths(before: &Value, next: &Value, prefix: &str, out: &mut Vec<String>) {
    match (before, next) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                diff_paths(
                    a.get(key).unwrap_or(&Value::Null),
                    b.get(key).unwrap_or(&Value::Null),
                    &format!("{prefix}.{key}"),
                    out,
                );
            }
        }
        _ if before == next => {}
        _ => out.push(prefix.to_string()),
    }
}

/// Env-won leaves are restored to the baseline so a form that sends them back
/// does not freeze the environment value into the row.
fn without_env_leaves(key: SettingsKey, mut value: Value, base: &Value, env: &[String]) -> Value {
    let prefix = format!("{}.", key.as_str());
    for path in env {
        let Some(leaf) = path.strip_prefix(&prefix) else {
            continue;
        };
        if let (Some(obj), Some(base_leaf)) = (value.as_object_mut(), base.get(leaf)) {
            obj.insert(leaf.to_string(), base_leaf.clone());
        }
    }
    value
}

/// Applies a settings change as one transaction: the actor's instance-admin
/// status is rechecked under a row lock, writers serialize on the settings
/// revision row, and the rows, revision and `instance_settings.updated`
/// event + audit commit together. Audit payloads carry key paths only, never
/// the values (source spec §10).
pub async fn apply_change(
    pool: &PgPool,
    actor: Uuid,
    ip: Option<&str>,
    brand_default: &str,
    change: SettingsChange,
) -> Result<Result<SettingsWriteOutcome, SettingsWriteError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !require_live_instance_admin(&mut tx, actor).await? {
        tx.rollback().await?;
        return Ok(Err(SettingsWriteError::NotAdmin));
    }
    set_system(&mut tx).await?;
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM fvoci.instance_settings_meta WHERE id = 1 FOR UPDATE",
    )
    .fetch_one(&mut *tx)
    .await?;
    let current = resolve(load_rows(&mut *tx).await?, revision, brand_default);

    let mut previous_asset = None;
    let mut extra = Map::new();
    let patch: Vec<(SettingsKey, Option<Value>)> = match change {
        SettingsChange::Patch(items) => items
            .into_iter()
            .map(|(key, value)| {
                let value = match (key, value) {
                    (SettingsKey::Branding, Some(mut v)) => {
                        v["logo"] = serde_json::to_value(&current.values.branding.logo)
                            .unwrap_or(Value::Null);
                        v["favicon"] = serde_json::to_value(&current.values.branding.favicon)
                            .unwrap_or(Value::Null);
                        Some(v)
                    }
                    (_, v) => v,
                };
                (key, value)
            })
            .collect(),
        SettingsChange::BrandingAsset { kind, asset } => {
            let mut branding = current.values.branding.clone();
            previous_asset = branding.asset(kind).cloned();
            if asset.is_none() && previous_asset.is_none() {
                tx.rollback().await?;
                return Ok(Err(SettingsWriteError::AssetMissing));
            }
            let key_for_audit = asset
                .as_ref()
                .or(previous_asset.as_ref())
                .map(|a| a.key.to_string());
            if let Some(key) = key_for_audit {
                extra.insert("assetKey".to_string(), Value::String(key));
            }
            branding.set_asset(kind, asset);
            vec![(
                SettingsKey::Branding,
                Some(serde_json::to_value(branding).unwrap_or(Value::Null)),
            )]
        }
    };

    let mut changed = Vec::new();
    let mut fields = Vec::new();
    let mut writes: Vec<(SettingsKey, Option<Value>)> = Vec::new();
    for (key, value) in patch {
        let row = current.stored.get(key.as_str());
        let default = SettingsValues::defaults(brand_default).get_json(key);
        let base = row.cloned().unwrap_or_else(|| default.clone());
        match value {
            None => {
                if row.is_none() {
                    continue;
                }
                diff_paths(&base, &default, key.as_str(), &mut fields);
                changed.push(key);
                writes.push((key, None));
            }
            Some(value) => {
                let value = without_env_leaves(key, value, &base, &current.env_applied);
                let mut paths = Vec::new();
                diff_paths(&base, &value, key.as_str(), &mut paths);
                // Without a row, write even a default-equal value: the admin
                // pins today's default.
                if row.is_some() && paths.is_empty() {
                    continue;
                }
                fields.extend(paths);
                changed.push(key);
                writes.push((key, Some(value)));
            }
        }
    }
    if changed.is_empty() {
        tx.rollback().await?;
        return Ok(Ok(SettingsWriteOutcome {
            changed,
            previous_asset: None,
            snapshot: current,
        }));
    }
    for (key, value) in &writes {
        match value {
            None => {
                sqlx::query("DELETE FROM fvoci.instance_settings WHERE key = $1")
                    .bind(key.as_str())
                    .execute(&mut *tx)
                    .await?;
            }
            Some(value) => {
                sqlx::query(
                    r#"
                    INSERT INTO fvoci.instance_settings (key, value, updated_at)
                    VALUES ($1, $2, now())
                    ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()
                    "#,
                )
                .bind(key.as_str())
                .bind(value)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    let revision: i64 = sqlx::query_scalar(
        "UPDATE fvoci.instance_settings_meta SET revision = revision + 1 WHERE id = 1 RETURNING revision",
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut payload = Map::new();
    payload.insert(
        "keys".to_string(),
        json!(changed.iter().map(|k| k.as_str()).collect::<Vec<_>>()),
    );
    payload.insert("fields".to_string(), json!(fields));
    payload.extend(extra);
    record_instance_change(
        &mut tx,
        InstanceChange {
            actor_user_id: actor,
            verb: "instance_settings.updated",
            target: None,
            payload: Value::Object(payload),
            ip,
        },
    )
    .await?;
    let snapshot = resolve(load_rows(&mut *tx).await?, revision, brand_default);
    tx.commit().await?;
    Ok(Ok(SettingsWriteOutcome {
        changed,
        previous_asset,
        snapshot,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_rows_fall_back_and_are_not_overridden() {
        let rows = vec![
            (
                "share".to_string(),
                json!({"enabled": false, "defaultExpiresDays": 3, "maxExpiresDays": 10}),
            ),
            ("auth".to_string(), json!({"passwordMinLength": 5})),
            ("unknown".to_string(), json!(1)),
        ];
        let snap = resolve(rows, 4, "FVOCI");
        assert!(!snap.values.share.enabled);
        assert_eq!(snap.values.auth.password_min_length, 10);
        assert_eq!(snap.overridden, vec![SettingsKey::Share]);
        assert!(snap.stored.contains_key("auth"));
        assert!(!snap.stored.contains_key("unknown"));
    }

    #[test]
    fn asset_href_is_a_delivery_path_not_the_storage_key() {
        let asset = BrandingAsset {
            key: Uuid::nil(),
            sha256: "a".repeat(64),
            mime: "image/png".to_string(),
        };
        let href = asset_href(BrandingAssetKind::Logo, Some(&asset)).unwrap();
        assert_eq!(href, "/api/v1/branding/logo?v=aaaaaaaaaaaa");
        assert!(!href.contains(&Uuid::nil().to_string()));
        assert_eq!(asset_href(BrandingAssetKind::Favicon, None), None);
    }

    #[test]
    fn diff_paths_reports_changed_leaves() {
        let mut out = Vec::new();
        diff_paths(
            &json!({"a": 1, "b": {"c": 2}}),
            &json!({"a": 1, "b": {"c": 3}}),
            "k",
            &mut out,
        );
        assert_eq!(out, vec!["k.b.c"]);
    }
}
