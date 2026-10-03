//! Operator key maintenance (source `fvoci secrets audit|rotate`,
//! packages/core/src/secret-maintenance.ts and
//! packages/db/src/pg/repos/secrets.ts), run by
//! `fvoci-migrate --secrets-audit|--secrets-rotate`.
//!
//! Both walk every value sealed with `ENCRYPTION_KEYS` (webhook signing
//! secrets, workspace SSO client secrets, TOTP secrets, the VAPID private key)
//! in id order, 100 rows per system transaction, with the same row-binding
//! AAD the product uses. Like the source they run as the app role in the
//! system context; no owner credentials are needed. Opened values are dropped
//! at once; reports carry only key ids and counts.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::context::set_system;
use crate::identity::{user_mfa_context, workspace_oidc_context};
use crate::integrations::webhooks::webhook_secret_context;
use crate::push::vapid_context;
use crate::secret_box::{self, SecretBoxError};

const BATCH: i64 = 100;
const VAPID_ID: Uuid = Uuid::nil();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretClass {
    Webhook,
    WorkspaceOidc,
    UserMfa,
    Vapid,
    Zotero,
}

impl SecretClass {
    pub const ALL: [SecretClass; 5] = [
        SecretClass::Webhook,
        SecretClass::WorkspaceOidc,
        SecretClass::UserMfa,
        SecretClass::Vapid,
        SecretClass::Zotero,
    ];

    /// Source target names, used as report labels.
    pub fn label(self) -> &'static str {
        match self {
            SecretClass::Webhook => "webhook",
            SecretClass::WorkspaceOidc => "workspace-oidc",
            SecretClass::UserMfa => "user-mfa",
            SecretClass::Vapid => "vapid",
            SecretClass::Zotero => "zotero",
        }
    }
}

/// One sealed value. `id` is the row key (the nil id for the VAPID key).
struct SealedRow {
    id: Uuid,
    context: String,
    stored: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SecretMaintenanceError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{class} {id} does not open: {reason}")]
    Unopenable {
        class: &'static str,
        id: Uuid,
        reason: SecretBoxError,
    },
    #[error("{0} sealed secret(s) do not open with ENCRYPTION_KEYS; nothing was changed (run --secrets-audit)")]
    Preflight(u64),
    #[error("secret rotation conflict on {class} {id}; retry after inspecting concurrent changes")]
    Conflict { class: &'static str, id: Uuid },
}

/// Source `secretKeyId`: the key id of a well-formed `enc:v2:<kid>:<body>`.
fn key_id(stored: &str) -> Option<&str> {
    let (kid, body) = stored.strip_prefix("enc:v2:")?.split_once(':')?;
    let url_safe = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    ((1..=32).contains(&kid.len()) && url_safe(kid) && !body.is_empty() && url_safe(body))
        .then_some(kid)
}

/// Source `secretMaintenance.list`: the next batch after `after` in id order.
async fn list(
    tx: &mut Transaction<'_, Postgres>,
    class: SecretClass,
    after: Option<Uuid>,
) -> Result<Vec<SealedRow>, sqlx::Error> {
    if class == SecretClass::Zotero {
        let rows: Vec<(Uuid, Uuid, Uuid, String)> = sqlx::query_as(
            "SELECT connector_id, workspace_id, owner_user_id, sealed_key FROM fvoci.zotero_credentials WHERE ($1::uuid IS NULL OR connector_id>$1) ORDER BY connector_id LIMIT $2"
        ).bind(after).bind(BATCH).fetch_all(&mut **tx).await?;
        return Ok(rows
            .into_iter()
            .map(|(id, tenant, owner, stored)| SealedRow {
                id,
                context: crate::integrations::zotero::secret_context(tenant, owner, id),
                stored,
            })
            .collect());
    }
    let rows: Vec<(Uuid, Uuid, String)> = match class {
        SecretClass::Zotero => unreachable!("zotero returned above"),
        SecretClass::Webhook => {
            sqlx::query_as(
                "SELECT id, workspace_id, secret FROM fvoci.webhooks \
                 WHERE ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2",
            )
            .bind(after)
            .bind(BATCH)
            .fetch_all(&mut **tx)
            .await?
        }
        SecretClass::WorkspaceOidc => {
            sqlx::query_as(
                "SELECT id, workspace_id, client_secret FROM fvoci.workspace_oidc \
                 WHERE ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2",
            )
            .bind(after)
            .bind(BATCH)
            .fetch_all(&mut **tx)
            .await?
        }
        SecretClass::UserMfa => {
            sqlx::query_as(
                "SELECT user_id, user_id, totp_secret FROM fvoci.user_mfa \
                 WHERE ($1::uuid IS NULL OR user_id > $1) ORDER BY user_id LIMIT $2",
            )
            .bind(after)
            .bind(BATCH)
            .fetch_all(&mut **tx)
            .await?
        }
        SecretClass::Vapid => {
            if after.is_some() {
                return Ok(Vec::new());
            }
            // The app role reads the sealed key only through the definer.
            let stored: Option<String> = sqlx::query_scalar("SELECT fvoci.app_vapid_private_key()")
                .fetch_one(&mut **tx)
                .await?;
            return Ok(stored
                .map(|stored| SealedRow {
                    id: VAPID_ID,
                    context: vapid_context().to_string(),
                    stored,
                })
                .into_iter()
                .collect());
        }
    };
    Ok(rows
        .into_iter()
        .map(|(id, owner, stored)| SealedRow {
            id,
            context: match class {
                SecretClass::Webhook => webhook_secret_context(owner, id),
                SecretClass::WorkspaceOidc => workspace_oidc_context(owner),
                SecretClass::UserMfa => user_mfa_context(owner),
                SecretClass::Vapid => unreachable!("vapid returned above"),
                SecretClass::Zotero => unreachable!("zotero returned above"),
            },
            stored,
        })
        .collect())
}

/// Source `secretMaintenance.replace`: compare-and-set on the stored value.
async fn replace(
    tx: &mut Transaction<'_, Postgres>,
    class: SecretClass,
    id: Uuid,
    expected: &str,
    stored: &str,
) -> Result<bool, sqlx::Error> {
    let sql = match class {
        SecretClass::Zotero => "UPDATE fvoci.zotero_credentials SET sealed_key=$3 WHERE connector_id=$1 AND sealed_key=$2",
        SecretClass::Webhook => {
            "UPDATE fvoci.webhooks SET secret = $3, updated_at = now() WHERE id = $1 AND secret = $2"
        }
        SecretClass::WorkspaceOidc => {
            "UPDATE fvoci.workspace_oidc SET client_secret = $3, updated_at = now() WHERE id = $1 AND client_secret = $2"
        }
        SecretClass::UserMfa => {
            "UPDATE fvoci.user_mfa SET totp_secret = $3, updated_at = now() WHERE user_id = $1 AND totp_secret = $2"
        }
        SecretClass::Vapid => {
            return sqlx::query_scalar("SELECT fvoci.app_replace_vapid_private($1, $2)")
                .bind(expected)
                .bind(stored)
                .fetch_one(&mut **tx)
                .await;
        }
    };
    let result = sqlx::query(sql)
        .bind(id)
        .bind(expected)
        .bind(stored)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() == 1)
}

async fn system_tx(pool: &PgPool) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    Ok(tx)
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretAudit {
    /// Source shape: `<class>:<key id>` (or `<class>:invalid`) → count.
    pub secrets: BTreeMap<String, u64>,
    /// Source shape: password pepper key id (or `unknown`) → count.
    pub passwords: BTreeMap<String, u64>,
    /// Values that do not open, plus password hashes whose pepper key is
    /// missing or whose format is invalid. Nonzero exits 1.
    pub problems: u64,
    /// `ENCRYPTION_ACTIVE_KEY_ID`.
    pub active_key_id: String,
    /// Sealed values that open but not under the active key (what
    /// `--secrets-rotate` would re-seal).
    pub not_active: u64,
    /// Key ids named by sealed values that `ENCRYPTION_KEYS` lacks.
    pub missing_key_ids: BTreeSet<String>,
    /// Pepper key ids named by password hashes that `PASSWORD_PEPPER_KEYS` lacks.
    pub missing_password_key_ids: BTreeSet<String>,
}

/// Source `auditSecrets`.
pub async fn audit_secrets(
    pool: &PgPool,
    ring: &Keyring,
    peppers: &Keyring,
) -> Result<SecretAudit, SecretMaintenanceError> {
    let mut report = SecretAudit {
        active_key_id: ring.active_id.clone(),
        ..Default::default()
    };
    for class in SecretClass::ALL {
        let mut after = None;
        loop {
            let mut tx = system_tx(pool).await?;
            let rows = list(&mut tx, class, after).await?;
            tx.commit().await?;
            let Some(last) = rows.last() else { break };
            after = Some(last.id);
            for row in &rows {
                let kid = key_id(&row.stored);
                let label = format!("{}:{}", class.label(), kid.unwrap_or("invalid"));
                *report.secrets.entry(label).or_default() += 1;
                match secret_box::open(ring, &row.stored, &row.context) {
                    Ok(_) if kid == Some(ring.active_id.as_str()) => {}
                    Ok(_) => report.not_active += 1,
                    Err(error) => {
                        report.problems += 1;
                        if let (SecretBoxError::KeyUnavailable, Some(kid)) = (error, kid) {
                            report.missing_key_ids.insert(kid.to_string());
                        }
                    }
                }
            }
        }
    }
    let mut tx = system_tx(pool).await?;
    let passwords: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT key_id, total, invalid FROM fvoci.app_password_key_inventory()")
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    for (kid, total, invalid) in passwords {
        let (total, invalid) = (total.max(0) as u64, invalid.max(0) as u64);
        if peppers.keys.contains_key(&kid) {
            report.problems += invalid;
        } else {
            report.problems += total;
            report.missing_password_key_ids.insert(kid.clone());
        }
        report.passwords.insert(kid, total);
    }
    Ok(report)
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct RotateReport {
    pub changed: u64,
    pub unchanged: u64,
}

/// Source `rotateSecrets`: re-seals every value not sealed with the active
/// key, 100 rows per system transaction, each write a compare-and-set on the
/// value read. Re-running is a no-op once everything is under the active key.
///
/// FVOCI difference: a read-only pass first opens every value and refuses to
/// write anything if one does not open (the source re-seals the batches
/// before the first failing one and then stops).
pub async fn rotate_secrets(
    pool: &PgPool,
    ring: &Keyring,
) -> Result<RotateReport, SecretMaintenanceError> {
    let preflight = audit_sealed(pool, ring).await?;
    if preflight > 0 {
        return Err(SecretMaintenanceError::Preflight(preflight));
    }
    let mut report = RotateReport::default();
    for class in SecretClass::ALL {
        let mut after = None;
        loop {
            let mut tx = system_tx(pool).await?;
            let rows = list(&mut tx, class, after).await?;
            let Some(last) = rows.last() else { break };
            after = Some(last.id);
            let mut changed: u64 = 0;
            for row in &rows {
                let plaintext =
                    secret_box::open(ring, &row.stored, &row.context).map_err(|reason| {
                        SecretMaintenanceError::Unopenable {
                            class: class.label(),
                            id: row.id,
                            reason,
                        }
                    })?;
                if key_id(&row.stored) == Some(ring.active_id.as_str()) {
                    continue;
                }
                let sealed =
                    secret_box::seal(ring, &plaintext, &row.context).map_err(|reason| {
                        SecretMaintenanceError::Unopenable {
                            class: class.label(),
                            id: row.id,
                            reason,
                        }
                    })?;
                drop(plaintext);
                if !replace(&mut tx, class, row.id, &row.stored, &sealed).await? {
                    return Err(SecretMaintenanceError::Conflict {
                        class: class.label(),
                        id: row.id,
                    });
                }
                changed += 1;
            }
            tx.commit().await?;
            report.changed += changed;
            report.unchanged += rows.len() as u64 - changed;
        }
    }
    Ok(report)
}

/// Number of sealed values that do not open with `ring`.
async fn audit_sealed(pool: &PgPool, ring: &Keyring) -> Result<u64, SecretMaintenanceError> {
    let mut failed = 0;
    for class in SecretClass::ALL {
        let mut after = None;
        loop {
            let mut tx = system_tx(pool).await?;
            let rows = list(&mut tx, class, after).await?;
            tx.commit().await?;
            let Some(last) = rows.last() else { break };
            after = Some(last.id);
            failed += rows
                .iter()
                .filter(|row| secret_box::open(ring, &row.stored, &row.context).is_err())
                .count() as u64;
        }
    }
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_matches_the_sealed_header_only() {
        assert_eq!(key_id("enc:v2:k1:AAAA"), Some("k1"));
        assert_eq!(key_id("enc:v2:a-b_c:AAAA"), Some("a-b_c"));
        assert_eq!(key_id("enc:v2::AAAA"), None);
        assert_eq!(key_id("enc:v2:bad.id:AAAA"), None);
        assert_eq!(key_id(&format!("enc:v2:{}:AAAA", "k".repeat(33))), None);
        assert_eq!(key_id("enc:v2:k1:"), None);
        assert_eq!(key_id("enc:v2:k1:AA:AA"), None);
        assert_eq!(key_id("plaintext"), None);
    }
}
