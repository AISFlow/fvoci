//! Post-restore check that every secret sealed with `ENCRYPTION_KEYS`
//! opens with the configured keyring (`fvoci-migrate --verify-secrets`).
//!
//! A restored database whose sealed values do not open is not usable: MFA
//! sign-in, workspace SSO and webhook deliveries would fail closed at the
//! first use. This opens every TOTP secret (`user_mfa`), workspace SSO client
//! secret (`workspace_oidc`), webhook signing secret (`webhooks`) and the Web
//! Push VAPID private key (`vapid`, reported under the nil id) with the same
//! AAD context the product uses. The opened values are dropped at once
//! and never reported.
//!
//! OIDC flow states (`oidc_states`) are not opened: they are single-use,
//! expire ten minutes after they were issued (so every state in a backup
//! taken from a stopped server is stale by the time it is restored), and a
//! state that does not open only fails that one sign-in attempt closed. The
//! app role cannot read the table either (definer functions only).
//!
//! Runs as the app role in the system context, like the server's own
//! background paths; no owner credentials are needed.

use std::collections::BTreeMap;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::context::set_system;
use crate::identity::{user_mfa_context, workspace_oidc_context};
use crate::integrations::webhooks::webhook_secret_context;
use crate::push::vapid_context;
use crate::secret_box::{self, SecretBoxError};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedTableReport {
    pub checked: u64,
    /// Row ids whose value names a key id the keyring does not have.
    pub key_unavailable: Vec<Uuid>,
    /// Row ids whose value is malformed or does not authenticate (wrong key
    /// under the same id, corrupted value, or copied from another row).
    pub invalid: Vec<Uuid>,
}

impl SealedTableReport {
    fn failed(&self) -> usize {
        self.key_unavailable.len() + self.invalid.len()
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsVerifyReport {
    /// Whether ENCRYPTION_KEYS is configured for this process.
    pub keyring_configured: bool,
    /// `user_mfa` TOTP secrets, by user id.
    pub user_mfa: SealedTableReport,
    /// `workspace_oidc` client secrets, by workspace id.
    pub workspace_oidc: SealedTableReport,
    /// `webhooks` signing secrets, by webhook id.
    pub webhooks: SealedTableReport,
    /// VAPID private key sealed as `vapid:1`.
    pub vapid: SealedTableReport,
    /// Owner-private Zotero connector credentials, never serialized values.
    pub zotero: SealedTableReport,
    /// How many sealed values name each key id (rotation: a key id still in
    /// use here must stay in the keyring).
    pub key_ids_in_use: BTreeMap<String, u64>,
    /// Not opened; see the module documentation.
    pub oidc_states: &'static str,
}

impl SecretsVerifyReport {
    pub fn failed(&self) -> usize {
        self.user_mfa.failed()
            + self.workspace_oidc.failed()
            + self.webhooks.failed()
            + self.vapid.failed()
            + self.zotero.failed()
    }

    pub fn is_complete(&self) -> bool {
        self.failed() == 0
    }
}

fn key_id(sealed: &str) -> Option<&str> {
    sealed.strip_prefix("enc:v2:")?.split(':').next()
}

fn check(
    keys: Option<&Keyring>,
    table: &mut SealedTableReport,
    in_use: &mut BTreeMap<String, u64>,
    id: Uuid,
    sealed: &str,
    context: &str,
) {
    table.checked += 1;
    if let Some(kid) = key_id(sealed) {
        *in_use.entry(kid.to_string()).or_default() += 1;
    }
    let Some(keys) = keys else {
        table.key_unavailable.push(id);
        return;
    };
    match secret_box::open(keys, sealed, context) {
        Ok(_) => {}
        Err(SecretBoxError::KeyUnavailable) => table.key_unavailable.push(id),
        Err(SecretBoxError::Invalid) => table.invalid.push(id),
    }
}

/// Opens every sealed value. `keys` is `None` when ENCRYPTION_KEYS is unset:
/// then any sealed value counts as key-unavailable.
pub async fn verify_sealed_secrets(
    pool: &PgPool,
    keys: Option<&Keyring>,
) -> Result<SecretsVerifyReport, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let mfa: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT user_id, totp_secret FROM fvoci.user_mfa ORDER BY user_id")
            .fetch_all(&mut *tx)
            .await?;
    let sso: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT workspace_id, client_secret FROM fvoci.workspace_oidc ORDER BY workspace_id",
    )
    .fetch_all(&mut *tx)
    .await?;
    let hooks: Vec<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT workspace_id, id, secret FROM fvoci.webhooks ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    // The app role reads the sealed VAPID key only through the definer.
    let vapid: Option<String> = sqlx::query_scalar("SELECT fvoci.app_vapid_private_key()")
        .fetch_one(&mut *tx)
        .await?;
    let zotero: Vec<(Uuid, Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT connector_id, workspace_id, owner_user_id, sealed_key FROM fvoci.zotero_credentials ORDER BY connector_id"
    ).fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let mut report = SecretsVerifyReport {
        keyring_configured: keys.is_some(),
        oidc_states: "skipped: single-use, expire within 10 minutes, fail closed per sign-in",
        ..Default::default()
    };
    let in_use = &mut report.key_ids_in_use;
    for (id, tenant, owner, sealed) in &zotero {
        check(
            keys,
            &mut report.zotero,
            in_use,
            *id,
            sealed,
            &crate::integrations::zotero::secret_context(*tenant, *owner, *id),
        );
    }

    for (user_id, sealed) in &mfa {
        let context = user_mfa_context(*user_id);
        check(
            keys,
            &mut report.user_mfa,
            in_use,
            *user_id,
            sealed,
            &context,
        );
    }
    for (workspace_id, sealed) in &sso {
        let context = workspace_oidc_context(*workspace_id);
        check(
            keys,
            &mut report.workspace_oidc,
            in_use,
            *workspace_id,
            sealed,
            &context,
        );
    }
    for (workspace_id, id, sealed) in &hooks {
        let context = webhook_secret_context(*workspace_id, *id);
        check(keys, &mut report.webhooks, in_use, *id, sealed, &context);
    }
    if let Some(sealed) = vapid {
        check(
            keys,
            &mut report.vapid,
            in_use,
            Uuid::nil(),
            &sealed,
            vapid_context(),
        );
    }
    Ok(report)
}

/// Post-restore verification through the configured driver. All ciphertext is
/// read in one system snapshot, opened with the product's AAD, and never emitted.
pub async fn verify_sealed_secrets_backend(
    backend: &crate::db::backend::Backend,
    keys: Option<&Keyring>,
) -> Result<SecretsVerifyReport, sqlx::Error> {
    use crate::db::backend::{Backend, OperationTx};
    use crate::db::codec::Cell;
    if let Backend::Postgres(pool) = backend {
        return verify_sealed_secrets(pool, keys).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_system().await?;
        let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
        family.require_system_context()?;
        let mut report = SecretsVerifyReport {
            keyring_configured: keys.is_some(),
            oidc_states: "skipped: single-use, expire within 10 minutes, fail closed per sign-in",
            ..Default::default()
        };
        for row in family.query("SELECT user_id,totp_secret FROM user_mfa ORDER BY user_id", &[]).await? {
            let id = row.cell(0)?.id()?;
            check(keys, &mut report.user_mfa, &mut report.key_ids_in_use, id, &row.cell(1)?.string()?, &user_mfa_context(id));
        }
        for row in family.query("SELECT workspace_id,client_secret FROM workspace_oidc ORDER BY workspace_id", &[]).await? {
            let id = row.cell(0)?.id()?;
            check(keys, &mut report.workspace_oidc, &mut report.key_ids_in_use, id, &row.cell(1)?.string()?, &workspace_oidc_context(id));
        }
        for row in family.query("SELECT workspace_id,id,secret FROM webhooks ORDER BY id", &[]).await? {
            let workspace = row.cell(0)?.id()?;
            let id = row.cell(1)?.id()?;
            check(keys, &mut report.webhooks, &mut report.key_ids_in_use, id, &row.cell(2)?.string()?, &webhook_secret_context(workspace,id));
        }
        for row in family.query("SELECT vapid_private_key FROM instance_config WHERE id=1", &[]).await? {
            if let Some(sealed) = row.cell(0)?.optional(Cell::string)? {
                check(keys, &mut report.vapid, &mut report.key_ids_in_use, Uuid::nil(), &sealed, vapid_context());
            }
        }
        for row in family.query("SELECT connector_id,workspace_id,owner_user_id,sealed_key FROM zotero_credentials ORDER BY connector_id", &[]).await? {
            let id = row.cell(0)?.id()?;
            check(keys, &mut report.zotero, &mut report.key_ids_in_use, id, &row.cell(3)?.string()?, &crate::integrations::zotero::secret_context(row.cell(1)?.id()?,row.cell(2)?.id()?,id));
        }
        Ok(report)
    }.await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        ));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(json: &str, active: &str) -> Keyring {
        Keyring::parse_named(json, active, "ENCRYPTION_KEYS").unwrap()
    }

    #[test]
    fn classifies_missing_key_ids_and_bad_values() {
        let a = ring(&format!(r#"{{"a":"{}"}}"#, "11".repeat(32)), "a");
        let b = ring(&format!(r#"{{"b":"{}"}}"#, "22".repeat(32)), "b");
        let a_other = ring(&format!(r#"{{"a":"{}"}}"#, "33".repeat(32)), "a");
        let id = Uuid::now_v7();
        let sealed = secret_box::seal(&a, "totp", "user-mfa:x").unwrap();
        let mut table = SealedTableReport::default();
        let mut in_use = BTreeMap::new();
        check(Some(&a), &mut table, &mut in_use, id, &sealed, "user-mfa:x");
        check(Some(&b), &mut table, &mut in_use, id, &sealed, "user-mfa:x");
        check(
            Some(&a_other),
            &mut table,
            &mut in_use,
            id,
            &sealed,
            "user-mfa:x",
        );
        check(Some(&a), &mut table, &mut in_use, id, &sealed, "user-mfa:y");
        check(None, &mut table, &mut in_use, id, &sealed, "user-mfa:x");
        assert_eq!(table.checked, 5);
        assert_eq!(table.key_unavailable, vec![id, id]);
        assert_eq!(table.invalid, vec![id, id]);
        assert_eq!(in_use.get("a"), Some(&5));
        let json = serde_json::to_string(&table).unwrap();
        assert!(!json.contains("totp"));
    }
}
