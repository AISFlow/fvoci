#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! `fvoci-migrate --secrets-audit|--secrets-rotate` (source `fvoci secrets
//! audit|rotate`) against a real PostgreSQL, run as the binary with the
//! NOSUPERUSER/NOBYPASSRLS app role the command is documented to use.

#[path = "support/project_harness.rs"]
mod project_harness;

use fvoci_server::auth::password::Keyring;
use fvoci_server::db::context::set_system;
use fvoci_server::identity::{user_mfa_context, workspace_oidc_context};
use fvoci_server::integrations::webhooks::webhook_secret_context;
use fvoci_server::push::{ensure_vapid_keys, load_vapid_key_pair, load_vapid_public_key};
use fvoci_server::secret_box;
use project_harness::{admin_pool, app_pool, setup_session, TestDb};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const K1_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const K2_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
const WEBHOOK_A: &str = "whsec-plain-alpha";
const WEBHOOK_B: &str = "whsec-plain-bravo";
const SSO_SECRET: &str = "sso-plain-charlie";
const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXPTOTPPLAIN";

fn ring_json(keys: &[(&str, &str)]) -> String {
    serde_json::to_string(
        &keys
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap()
}

fn ring(keys: &[(&str, &str)], active: &str) -> Keyring {
    Keyring::parse_named(&ring_json(keys), active, "ENCRYPTION_KEYS").unwrap()
}

struct Run {
    ok: bool,
    report: Value,
    output: String,
}

async fn run(flag: &str, db_url: &str, keys: &[(&str, &str)], active: &str) -> Run {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg(flag)
        .env_clear()
        .env("DATABASE_APP_URL", db_url)
        .env("ENCRYPTION_KEYS", ring_json(keys))
        .env("ENCRYPTION_ACTIVE_KEY_ID", active)
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .output()
        .await
        .expect("run fvoci-migrate");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    Run {
        ok: out.status.success(),
        report: serde_json::from_str(stdout.trim()).unwrap_or(Value::Null),
        output: format!("{stdout}{stderr}"),
    }
}

/// Nothing secret reaches stdout/stderr: no plaintext, no key material.
fn assert_no_secrets(run: &Run) {
    for needle in [
        WEBHOOK_A,
        WEBHOOK_B,
        SSO_SECRET,
        TOTP_SECRET,
        K1_HEX,
        K2_HEX,
        "aaaaaaaaaaaaaaaa",
        "argon2id",
        "enc:v2:",
    ] {
        assert!(
            !run.output.contains(needle),
            "output leaks {needle}: {}",
            run.output
        );
    }
}

struct Seeded {
    workspace_id: Uuid,
    owner_id: Uuid,
    webhook_a: Uuid,
    webhook_b: Uuid,
    oidc_id: Uuid,
    vapid_public: String,
}

async fn insert_webhook(
    admin: &PgPool,
    keys: &Keyring,
    workspace_id: Uuid,
    owner_id: Uuid,
    plaintext: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    let sealed =
        secret_box::seal(keys, plaintext, &webhook_secret_context(workspace_id, id)).unwrap();
    sqlx::query("INSERT INTO fvoci.webhooks (id, workspace_id, url, secret, events, created_by) VALUES ($1, $2, 'https://hooks.example/x', $3, ARRAY['document.created'], $4)")
        .bind(id)
        .bind(workspace_id)
        .bind(&sealed)
        .bind(owner_id)
        .execute(admin)
        .await
        .unwrap();
    id
}

/// One value of every class under k1, plus a second webhook already under k2.
async fn seed(harness: &TestDb, admin: &PgPool, app: &PgPool) -> Seeded {
    let (_router, _cookie, owner_id, workspace_id) = setup_session(harness).await;
    let k1 = ring(&[("k1", K1_HEX)], "k1");
    let k2 = ring(&[("k1", K1_HEX), ("k2", K2_HEX)], "k2");
    let webhook_a = insert_webhook(admin, &k1, workspace_id, owner_id, WEBHOOK_A).await;
    let webhook_b = insert_webhook(admin, &k2, workspace_id, owner_id, WEBHOOK_B).await;
    let oidc_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspace_oidc (id, workspace_id, issuer, client_id, client_secret) VALUES ($1, $2, 'https://idp.example', 'client', $3)")
        .bind(oidc_id)
        .bind(workspace_id)
        .bind(secret_box::seal(&k1, SSO_SECRET, &workspace_oidc_context(workspace_id)).unwrap())
        .execute(admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.user_mfa (user_id, totp_secret) VALUES ($1, $2)")
        .bind(owner_id)
        .bind(secret_box::seal(&k1, TOTP_SECRET, &user_mfa_context(owner_id)).unwrap())
        .execute(admin)
        .await
        .unwrap();
    ensure_vapid_keys(app, Some(&k1)).await.unwrap();
    let vapid_public = load_vapid_public_key(app).await.unwrap().unwrap();
    Seeded {
        workspace_id,
        owner_id,
        webhook_a,
        webhook_b,
        oidc_id,
        vapid_public,
    }
}

async fn stored_values(admin: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT v FROM (SELECT secret AS v FROM fvoci.webhooks UNION ALL SELECT client_secret FROM fvoci.workspace_oidc UNION ALL SELECT totp_secret FROM fvoci.user_mfa UNION ALL SELECT vapid_private_key FROM fvoci.instance_config WHERE vapid_private_key IS NOT NULL) s ORDER BY v",
    )
    .fetch_all(admin)
    .await
    .unwrap()
}

async fn password_users(admin: &PgPool) -> u64 {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.users WHERE password_hash IS NOT NULL")
        .fetch_one(admin)
        .await
        .unwrap() as u64
}

/// Every value opens under `keys` and still holds its original plaintext.
async fn assert_plaintext_preserved(admin: &PgPool, app: &PgPool, s: &Seeded, keys: &Keyring) {
    let webhook = |id: Uuid| async move {
        let stored: String = sqlx::query_scalar("SELECT secret FROM fvoci.webhooks WHERE id = $1")
            .bind(id)
            .fetch_one(admin)
            .await
            .unwrap();
        secret_box::open(keys, &stored, &webhook_secret_context(s.workspace_id, id)).unwrap()
    };
    assert_eq!(webhook(s.webhook_a).await, WEBHOOK_A);
    assert_eq!(webhook(s.webhook_b).await, WEBHOOK_B);
    let sso: String =
        sqlx::query_scalar("SELECT client_secret FROM fvoci.workspace_oidc WHERE id = $1")
            .bind(s.oidc_id)
            .fetch_one(admin)
            .await
            .unwrap();
    assert_eq!(
        secret_box::open(keys, &sso, &workspace_oidc_context(s.workspace_id)).unwrap(),
        SSO_SECRET
    );
    let totp: String =
        sqlx::query_scalar("SELECT totp_secret FROM fvoci.user_mfa WHERE user_id = $1")
            .bind(s.owner_id)
            .fetch_one(admin)
            .await
            .unwrap();
    assert_eq!(
        secret_box::open(keys, &totp, &user_mfa_context(s.owner_id)).unwrap(),
        TOTP_SECRET
    );
    // The VAPID pair still loads and matches the unchanged public key.
    assert!(load_vapid_key_pair(app, Some(keys))
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        load_vapid_public_key(app).await.unwrap().as_deref(),
        Some(s.vapid_public.as_str())
    );
}

#[tokio::test]
async fn audit_rotate_audit_clean_and_rotate_again_is_a_no_op() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let app = app_pool(&harness).await;
    let s = seed(&harness, &admin, &app).await;
    let both = [("k1", K1_HEX), ("k2", K2_HEX)];
    let passwords = password_users(&admin).await;
    assert!(passwords >= 1);

    // Two keys in use, k2 active: four values still under k1.
    let audit = run("--secrets-audit", &harness.app_url, &both, "k2").await;
    assert!(audit.ok, "{}", audit.output);
    assert_no_secrets(&audit);
    assert_eq!(
        audit.report,
        json!({
            "secrets": {
                "user-mfa:k1": 1,
                "vapid:k1": 1,
                "webhook:k1": 1,
                "webhook:k2": 1,
                "workspace-oidc:k1": 1,
            },
            "passwords": { "test": passwords },
            "problems": 0,
            "activeKeyId": "k2",
            "notActive": 4,
            "missingKeyIds": [],
            "missingPasswordKeyIds": [],
        })
    );

    let rotate = run("--secrets-rotate", &harness.app_url, &both, "k2").await;
    assert!(rotate.ok, "{}", rotate.output);
    assert_no_secrets(&rotate);
    assert_eq!(rotate.report, json!({ "changed": 4, "unchanged": 1 }));

    // Everything is under k2 now: k1 can be dropped from the keyring.
    let only_k2 = [("k2", K2_HEX)];
    let audit = run("--secrets-audit", &harness.app_url, &only_k2, "k2").await;
    assert!(audit.ok, "{}", audit.output);
    assert_eq!(audit.report["problems"], 0);
    assert_eq!(audit.report["notActive"], 0);
    assert_eq!(
        audit.report["secrets"],
        json!({
            "user-mfa:k2": 1,
            "vapid:k2": 1,
            "webhook:k2": 2,
            "workspace-oidc:k2": 1,
        })
    );
    assert_plaintext_preserved(&admin, &app, &s, &ring(&only_k2, "k2")).await;

    // Idempotent: a second rotate changes nothing.
    let before = stored_values(&admin).await;
    let again = run("--secrets-rotate", &harness.app_url, &only_k2, "k2").await;
    assert!(again.ok, "{}", again.output);
    assert_eq!(again.report, json!({ "changed": 0, "unchanged": 5 }));
    assert_eq!(stored_values(&admin).await, before);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn rotate_with_a_missing_old_key_fails_closed_without_writing() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let app = app_pool(&harness).await;
    let s = seed(&harness, &admin, &app).await;
    const K3_HEX: &str = "3333333333333333333333333333333333333333333333333333333333333333";
    // 120 more k1 webhooks span two batches, and the TOTP secret (a later
    // class) is sealed with k3. The source would commit the webhook and SSO
    // batches before failing on it; here nothing may be written.
    let k1 = ring(&[("k1", K1_HEX)], "k1");
    for n in 0..120 {
        insert_webhook(
            &admin,
            &k1,
            s.workspace_id,
            s.owner_id,
            &format!("bulk-{n}"),
        )
        .await;
    }
    let k3 = ring(&[("k3", K3_HEX)], "k3");
    sqlx::query("UPDATE fvoci.user_mfa SET totp_secret = $2 WHERE user_id = $1")
        .bind(s.owner_id)
        .bind(secret_box::seal(&k3, TOTP_SECRET, &user_mfa_context(s.owner_id)).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let before = stored_values(&admin).await;

    // k3 is not in the keyring: fail closed.
    let both = [("k1", K1_HEX), ("k2", K2_HEX)];
    let rotate = run("--secrets-rotate", &harness.app_url, &both, "k2").await;
    assert!(!rotate.ok, "{}", rotate.output);
    assert_no_secrets(&rotate);
    assert!(
        rotate.output.contains("nothing was changed"),
        "{}",
        rotate.output
    );
    assert_eq!(stored_values(&admin).await, before);

    // The audit names the missing key and exits 1, still printing its report.
    let audit = run("--secrets-audit", &harness.app_url, &both, "k2").await;
    assert!(!audit.ok);
    assert_no_secrets(&audit);
    assert_eq!(audit.report["problems"], 1);
    assert_eq!(audit.report["missingKeyIds"], json!(["k3"]));
    assert_eq!(audit.report["secrets"]["webhook:k1"], 121);
    assert_eq!(audit.report["secrets"]["user-mfa:k3"], 1);
    assert_eq!(audit.report["notActive"], 123);

    // With the key restored the same rotate completes, across batches.
    let all = [("k1", K1_HEX), ("k2", K2_HEX), ("k3", K3_HEX)];
    let rotate = run("--secrets-rotate", &harness.app_url, &all, "k2").await;
    assert!(rotate.ok, "{}", rotate.output);
    assert_eq!(rotate.report, json!({ "changed": 124, "unchanged": 1 }));
    assert_plaintext_preserved(&admin, &app, &s, &ring(&[("k2", K2_HEX)], "k2")).await;

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn audit_counts_password_pepper_problems_and_refuses_privileged_roles() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let app = app_pool(&harness).await;
    let _ = setup_session(&harness).await;
    let passwords = password_users(&admin).await;
    // A hash under a retired pepper key (deleted user: still audited, like
    // the source) and one in no known format.
    let retired = format!(
        "$fvoci-pepper=old$argon2id$v=19$m=65536,t=3,p=1${}A${}A",
        "a".repeat(42),
        "b".repeat(42)
    );
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name, password_hash, deleted_at) VALUES ($1, 'old@example.com', 'Old', $2, now())")
        .bind(Uuid::now_v7())
        .bind(&retired)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name, password_hash) VALUES ($1, 'raw@example.com', 'Raw', 'not-a-hash')")
        .bind(Uuid::now_v7())
        .execute(&admin)
        .await
        .unwrap();

    let keys = [("k1", K1_HEX)];
    let audit = run("--secrets-audit", &harness.app_url, &keys, "k1").await;
    assert!(!audit.ok);
    assert_no_secrets(&audit);
    assert_eq!(
        audit.report["passwords"],
        json!({ "old": 1, "test": passwords, "unknown": 1 })
    );
    assert_eq!(audit.report["problems"], 2);
    assert_eq!(
        audit.report["missingPasswordKeyIds"],
        json!(["old", "unknown"])
    );

    // The definers answer only in the system context and expose counts.
    assert!(
        sqlx::query("SELECT * FROM fvoci.app_password_key_inventory()")
            .fetch_all(&app)
            .await
            .is_err()
    );
    let mut tx = app.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    let columns: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT * FROM fvoci.app_password_key_inventory() ORDER BY 1")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        columns,
        vec![
            ("old".into(), 1, 0),
            ("test".into(), passwords as i64, 0),
            ("unknown".into(), 1, 1)
        ]
    );
    assert!(
        sqlx::query_scalar::<_, Option<String>>("SELECT password_hash FROM fvoci.users LIMIT 1")
            .fetch_one(&app)
            .await
            .is_err(),
        "app role must still not read password hashes"
    );

    // The commands refuse the owner/superuser URL (source assertRuntimeDatabase).
    let owner = run("--secrets-audit", &harness.admin_url, &keys, "k1").await;
    assert!(!owner.ok);
    assert!(owner.output.contains("non-superuser"), "{}", owner.output);
    let owner = run("--secrets-rotate", &harness.admin_url, &keys, "k1").await;
    assert!(!owner.ok);
    assert!(owner.output.contains("non-superuser"), "{}", owner.output);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn vapid_replace_is_a_system_only_compare_and_set() {
    let harness = TestDb::bootstrap().await;
    let app = app_pool(&harness).await;
    let admin = admin_pool(&harness).await;
    let k1 = ring(&[("k1", K1_HEX)], "k1");
    ensure_vapid_keys(&app, Some(&k1)).await.unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT vapid_private_key FROM fvoci.instance_config WHERE id = 1")
            .fetch_one(&admin)
            .await
            .unwrap();

    assert!(
        sqlx::query("SELECT fvoci.app_replace_vapid_private($1, 'enc:v2:k2:AAAA')")
            .bind(&stored)
            .execute(&app)
            .await
            .is_err(),
        "requires system context"
    );
    let mut tx = app.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    // A concurrent --rotate-vapid stored another value: the CAS is a no-op.
    let replaced: bool = sqlx::query_scalar(
        "SELECT fvoci.app_replace_vapid_private('enc:v2:k1:stale', 'enc:v2:k2:AAAA')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!replaced);
    let replaced: bool =
        sqlx::query_scalar("SELECT fvoci.app_replace_vapid_private($1, 'enc:v2:k2:AAAA')")
            .bind(&stored)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(replaced);
    tx.rollback().await.unwrap();
    let after: String =
        sqlx::query_scalar("SELECT vapid_private_key FROM fvoci.instance_config WHERE id = 1")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(after, stored);

    app.close().await;
    admin.close().await;
    harness.cleanup().await;
}
