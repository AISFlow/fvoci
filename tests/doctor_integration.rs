#![cfg(feature = "db-tests")]
//! `fvoci-migrate --doctor` as a real child process against a real database
//! and app role: a healthy install passes (exit 0), and each broken setting is
//! reported by name with exit 1 and without secrets in the output.

#[allow(dead_code)]
mod support;

use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use support::{TestDb, PEPPER};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use uuid::Uuid;

/// A non-public pepper (the test `PEPPER` is the published dev key).
const REAL_PEPPER: &str =
    r#"{"install":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}"#;

/// Minimal SMTP responder: greeting, EHLO without STARTTLS, NOOP/RSET, QUIT.
async fn smtp_responder() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 doctor.test ESMTP\r\n").await;
                while let Ok(Some(line)) = lines.next_line().await {
                    let verb = line
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    let reply: &[u8] = match verb.as_str() {
                        "EHLO" | "HELO" => b"250 doctor.test\r\n",
                        "QUIT" => {
                            let _ = write.write_all(b"221 bye\r\n").await;
                            return;
                        }
                        _ => b"250 ok\r\n",
                    };
                    if write.write_all(reply).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (port, handle)
}

async fn doctor(envs: &[(&str, String)]) -> (i32, Value, String) {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"));
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .arg("--doctor")
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = tokio::time::timeout(Duration::from_secs(90), cmd.output())
        .await
        .expect("doctor finished")
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let report: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|_| panic!("doctor stdout is JSON: {stdout} / {stderr}"));
    (
        out.status.code().unwrap_or(-1),
        report,
        format!("{stdout}{stderr}"),
    )
}

fn check<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} check in {report}"))
}

fn passthrough(envs: &mut Vec<(&'static str, String)>, names: &[&'static str]) {
    for name in names {
        if let Ok(v) = std::env::var(name) {
            if !v.trim().is_empty() {
                envs.push((name, v));
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_passes_a_healthy_install_and_names_each_broken_setting() {
    let harness = TestDb::bootstrap().await;
    let storage = std::env::temp_dir().join(format!("fvoci-doctor-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage).unwrap();
    let (smtp_port, smtp) = smtp_responder().await;

    let mut healthy: Vec<(&'static str, String)> = vec![
        ("DATABASE_APP_URL", harness.app_url.clone()),
        ("PASSWORD_PEPPER_KEYS", REAL_PEPPER.to_string()),
        ("PASSWORD_PEPPER_ACTIVE_KEY_ID", "install".to_string()),
        ("FVOCI_PUBLIC_ORIGIN", "http://127.0.0.1:8080".to_string()),
        ("FVOCI_STORAGE_DIR", storage.display().to_string()),
        ("SMTP_HOST", "127.0.0.1".to_string()),
        ("SMTP_PORT", smtp_port.to_string()),
        ("SMTP_FROM", "doctor@example.com".to_string()),
    ];
    // Helpers and Meili are checked when this environment provides them (CI
    // sets the convert helper and Meili; the collab job sets the engine).
    passthrough(
        &mut healthy,
        &[
            "FVOCI_DOCUMENT_CONVERT_BIN",
            "FVOCI_COLLAB_ENGINE",
            "FVOCI_MEILI_URL",
            "FVOCI_MEILI_KEY",
        ],
    );
    let (code, report, _) = doctor(&healthy).await;
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["ok"], true);
    for name in [
        "env",
        "password_pepper_keys",
        "public_origin",
        "encryption_keys",
        "identity",
        "integrations",
        "database",
        "app_role",
        "schema_version",
        "pg_connection_budget",
        "storage",
        "meilisearch",
        "smtp",
        "document_convert",
        "extractor",
    ] {
        assert_eq!(check(&report, name)["ok"], true, "{name}: {report}");
    }
    assert_eq!(
        check(&report, "extractor")["detail"],
        "disabled (FVOCI_EXTRACTOR_BIN unset)"
    );
    // Probes provided by the environment actually ran (not "disabled").
    for (var, name) in [
        ("FVOCI_DOCUMENT_CONVERT_BIN", "document_convert"),
        ("FVOCI_MEILI_URL", "meilisearch"),
    ] {
        if healthy.iter().any(|(k, _)| *k == var) {
            assert!(
                check(&report, name).get("detail").is_none(),
                "{name}: {report}"
            );
        }
    }
    if healthy.iter().any(|(k, _)| *k == "FVOCI_COLLAB_ENGINE") {
        assert_eq!(check(&report, "collab_engine")["ok"], true, "{report}");
    }
    assert!(check(&report, "smtp").get("detail").is_none(), "{report}");

    // Broken: superuser DB role, published dev pepper, plain-http public
    // origin, storage path that is a file, missing collab engine, refused SMTP.
    let closed_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let not_a_dir = storage.join("file");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let broken: Vec<(&'static str, String)> = vec![
        ("DATABASE_APP_URL", harness.admin_url.clone()),
        ("PASSWORD_PEPPER_KEYS", PEPPER.to_string()),
        ("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test".to_string()),
        (
            "FVOCI_PUBLIC_ORIGIN",
            "http://fvoci.example.com".to_string(),
        ),
        ("FVOCI_STORAGE_DIR", not_a_dir.display().to_string()),
        (
            "FVOCI_COLLAB_ENGINE",
            "/nonexistent/collab-engine".to_string(),
        ),
        ("SMTP_HOST", "127.0.0.1".to_string()),
        ("SMTP_PORT", closed_port.to_string()),
        ("SMTP_FROM", "doctor@example.com".to_string()),
    ];
    let (code, report, output) = doctor(&broken).await;
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["ok"], false);
    for name in [
        "password_pepper_keys",
        "public_origin",
        "app_role",
        "storage",
        "collab_engine",
        "smtp",
    ] {
        let c = check(&report, name);
        assert_eq!(c["ok"], false, "{name}: {report}");
        assert!(
            c["detail"].as_str().is_some_and(|d| !d.is_empty()),
            "{name}"
        );
    }
    assert!(check(&report, "public_origin")["detail"]
        .as_str()
        .unwrap()
        .contains("https"));
    assert!(check(&report, "collab_engine")["detail"]
        .as_str()
        .unwrap()
        .contains("collaboration would be disabled"));

    // Unreachable database: reported, with the password masked.
    let admin = url::Url::parse(&harness.admin_url).unwrap();
    let password = admin.password().unwrap_or("").to_string();
    let mut unreachable = admin.clone();
    unreachable.set_port(Some(closed_port)).unwrap();
    let mut envs = healthy.clone();
    envs[0] = ("DATABASE_APP_URL", unreachable.to_string());
    let (code, report, unreachable_output) = doctor(&envs).await;
    assert_eq!(code, 1);
    assert_eq!(check(&report, "database")["ok"], false, "{report}");
    for secret in [password.as_str(), "0123456789abcdef0123456789abcdef"] {
        if !secret.is_empty() {
            assert!(!output.contains(secret), "secret in broken output");
            assert!(!unreachable_output.contains(secret), "secret in output");
        }
    }

    // Missing storage root: reported as failing, and the doctor creates nothing.
    let missing = storage.join(format!("missing-{}", Uuid::now_v7().simple()));
    let mut envs = healthy.clone();
    for pair in envs.iter_mut() {
        if pair.0 == "FVOCI_STORAGE_DIR" {
            pair.1 = missing.display().to_string();
        }
    }
    let (code, report, _) = doctor(&envs).await;
    assert_eq!(code, 1, "{report}");
    assert_eq!(check(&report, "storage")["ok"], false, "{report}");
    assert!(
        !missing.exists(),
        "the doctor must not create the storage root"
    );

    // Missing required env: a failing env check, still valid JSON and exit 1.
    let (code, report, _) = doctor(&[]).await;
    assert_eq!(code, 1);
    assert_eq!(check(&report, "env")["ok"], false);

    smtp.abort();
    let _ = std::fs::remove_dir_all(&storage);
    harness.cleanup().await.unwrap();
}
