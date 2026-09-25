#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::attachments::LocalStorage;
use fvoci_server::auth::token::hash_token;
use fvoci_server::db::magic::issue_password_reset_token;
use fvoci_server::db::outbox::mark_processed;
use fvoci_server::jobs::{
    run_daily_sweep, run_ics_token_gc, run_magic_token_gc, run_notification_gc, run_processed_gc,
    run_workspace_purge, spawn_maintenance, JobClaim, MaintenanceSettings, JOB_KEY_DAILY,
};
use fvoci_server::mail::{send_due_digests, Mailer, SmtpConfig};
use project_harness::{admin_pool, app_pool, json_request, setup_session, TestDb};
use serde_json::json;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct CapturedMail {
    to: String,
    data: String,
}

struct SmtpSink {
    port: u16,
    mails: Arc<Mutex<Vec<CapturedMail>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl SmtpSink {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind smtp");
        let port = listener.local_addr().expect("addr").port();
        let mails = Arc::new(Mutex::new(Vec::new()));
        let captured = mails.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let captured = captured.clone();
                tokio::spawn(async move {
                    let _ = serve_smtp(socket, captured).await;
                });
            }
        });
        Self {
            port,
            mails,
            handle,
        }
    }

    fn count(&self) -> usize {
        self.mails.lock().expect("mails").len()
    }

    /// Decoded plain body of the last captured mail.
    fn last_text(&self) -> String {
        let mails = self.mails.lock().expect("mails");
        let Some(mail) = mails.last() else {
            return String::new();
        };
        mailparse::parse_mail(mail.data.as_bytes())
            .and_then(|parsed| parsed.get_body())
            .unwrap_or_default()
    }
}

impl Drop for SmtpSink {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn serve_smtp(
    socket: tokio::net::TcpStream,
    captured: Arc<Mutex<Vec<CapturedMail>>>,
) -> Result<(), std::io::Error> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);
    writer.write_all(b"220 fvoci-test\r\n").await?;
    let mut rcpt = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let command = line.trim_end_matches(['\r', '\n']);
        let upper = command.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer.write_all(b"250 fvoci\r\n").await?;
        } else if upper.starts_with("MAIL FROM:") {
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            rcpt = command
                .split(':')
                .nth(1)
                .unwrap_or("")
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper == "DATA" {
            writer.write_all(b"354 go\r\n").await?;
            let mut data = String::new();
            loop {
                let mut data_line = String::new();
                reader.read_line(&mut data_line).await?;
                if data_line == ".\r\n" || data_line == ".\n" {
                    break;
                }
                data.push_str(&data_line);
            }
            captured.lock().expect("mails").push(CapturedMail {
                to: rcpt.clone(),
                data,
            });
            writer.write_all(b"250 ok\r\n").await?;
        } else if upper == "QUIT" {
            writer.write_all(b"221 bye\r\n").await?;
            break;
        } else {
            writer.write_all(b"250 ok\r\n").await?;
        }
    }
    Ok(())
}

fn mailer_for(port: u16) -> Mailer {
    Mailer::from_smtp(Some(SmtpConfig {
        host: "127.0.0.1".into(),
        port,
        from: "noreply@example.com".into(),
    }))
}

fn temp_storage() -> (PathBuf, LocalStorage) {
    let root = std::env::temp_dir().join(format!("fvoci-jobs-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).expect("storage root");
    (root.clone(), LocalStorage::new(root))
}

fn write_object(root: &Path, key: &str) {
    let dir = root.join("objects").join(key);
    std::fs::create_dir_all(&dir).expect("object dir");
    std::fs::write(dir.join("payload"), b"blob").expect("payload");
}

fn object_exists(root: &Path, key: &str) -> bool {
    root.join("objects").join(key).join("payload").exists()
}

async fn insert_attachment_with_key(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    uploader_id: Uuid,
    storage_key: &str,
) -> Uuid {
    let attachment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', 'probe.bin', 4, 4, $5, now())
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(uploader_id)
    .bind(storage_key)
    .execute(admin)
    .await
    .expect("insert attachment");
    attachment_id
}

async fn create_team_workspace(app: axum::Router, cookie: &str, name: &str, slug: &str) -> Uuid {
    let (status, created) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": name, "slug": slug})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created:?}");
    Uuid::parse_str(created["id"].as_str().unwrap()).unwrap()
}

async fn trash_workspace(app: axum::Router, cookie: &str, id: Uuid, slug: &str) {
    let (status, body) = json_request(
        app,
        "DELETE",
        &format!("/api/v1/workspaces/{id}"),
        Some(json!({"confirmSlug": slug})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body:?}");
}

#[tokio::test]
async fn workspace_purge_deletes_storage_and_is_idempotent() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let doomed_id = create_team_workspace(app.clone(), &cookie, "Doomed", "doomed-team").await;
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{doomed_id}/documents"),
        Some(json!({"title": "첨부 부모", "parentId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{wiki:?}");
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let key = Uuid::now_v7().to_string();
    write_object(&root, &key);
    insert_attachment_with_key(&admin, doomed_id, document_id, owner_id, &key).await;
    trash_workspace(app, &cookie, doomed_id, "doomed-team").await;

    let now = Utc::now();
    let cancel = CancellationToken::new();
    let fresh = run_workspace_purge(&pool, &storage, now, &cancel)
        .await
        .expect("fresh");
    assert_eq!(fresh.purged, 0);
    assert!(object_exists(&root, &key));

    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = $2 WHERE id = $1")
        .bind(doomed_id)
        .bind(now - ChronoDuration::days(31))
        .execute(&admin)
        .await
        .unwrap();

    let expired = run_workspace_purge(&pool, &storage, now, &cancel)
        .await
        .expect("expired");
    assert_eq!(expired.purged, 1);
    assert_eq!(expired.storage_deleted, 1);
    assert!(!object_exists(&root, &key));
    let gone: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(doomed_id)
        .fetch_optional(&admin)
        .await
        .unwrap();
    assert!(gone.is_none());

    let again = run_workspace_purge(&pool, &storage, now, &cancel)
        .await
        .expect("idempotent");
    assert_eq!(again.purged, 0);

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn ics_and_magic_token_gc_are_bounded_and_idempotent() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let now = Utc::now();
    let cancel = CancellationToken::new();

    let expired_ics = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.ics_tokens (id, workspace_id, user_id, token_hash, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(expired_ics)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(format!("expired-{}", expired_ics))
    .bind(now - ChronoDuration::hours(1))
    .execute(&admin)
    .await
    .expect("expired ics");

    let first = run_ics_token_gc(&pool, now, &cancel).await.expect("ics");
    assert_eq!(first, 1);
    let second = run_ics_token_gc(&pool, now, &cancel)
        .await
        .expect("ics again");
    assert_eq!(second, 0);

    let live_ics = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.ics_tokens (id, workspace_id, user_id, token_hash, expires_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(live_ics)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(format!("live-{}", live_ics))
    .bind(now + ChronoDuration::days(1))
    .execute(&admin)
    .await
    .expect("live ics");
    let live_kept = run_ics_token_gc(&pool, now, &cancel)
        .await
        .expect("live ics");
    assert_eq!(live_kept, 0);
    let live_left: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.ics_tokens WHERE id = $1")
        .bind(live_ics)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(live_left, 1);

    let live_hash = hash_token("live-magic-token-value");
    let expired_hash = hash_token("expired-magic-token-value");
    let generation: i32 =
        sqlx::query_scalar("SELECT auth_generation FROM fvoci.users WHERE id = $1")
            .bind(owner_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    issue_password_reset_token(
        &pool,
        owner_id,
        generation,
        &live_hash,
        now + ChronoDuration::minutes(10),
    )
    .await
    .expect("live magic");
    issue_password_reset_token(
        &pool,
        owner_id,
        generation,
        &expired_hash,
        now + ChronoDuration::minutes(10),
    )
    .await
    .expect("expired magic");
    sqlx::query("UPDATE fvoci.magic_tokens SET expires_at = $2 WHERE token_hash = $1")
        .bind(&expired_hash)
        .bind(now - ChronoDuration::minutes(1))
        .execute(&admin)
        .await
        .unwrap();

    let magic_first = run_magic_token_gc(&pool, now, &cancel)
        .await
        .expect("magic");
    assert_eq!(magic_first, 1);
    let magic_again = run_magic_token_gc(&pool, now, &cancel)
        .await
        .expect("magic again");
    assert_eq!(magic_again, 0);
    let live_magic: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.magic_tokens WHERE token_hash = $1")
            .bind(&live_hash)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(live_magic, 1);

    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn notification_and_processed_gc_honor_source_windows() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let cancel = CancellationToken::new();

    let old_read = Uuid::now_v7();
    let fresh_read = Uuid::now_v7();
    let old_archived = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.notifications (
            id, workspace_id, user_id, event_id, verb, payload, read_at, archived_at, created_at
        ) VALUES
            ($1, $4, $5, $6, 'task.updated', '{}'::jsonb, $7, NULL, $7),
            ($2, $4, $5, $8, 'task.updated', '{}'::jsonb, now(), NULL, now()),
            ($3, $4, $5, $9, 'task.updated', '{}'::jsonb, NULL, $7, $7)
        "#,
    )
    .bind(old_read)
    .bind(fresh_read)
    .bind(old_archived)
    .bind(workspace_id)
    .bind(owner_id)
    .bind(Uuid::now_v7())
    .bind(Utc::now() - ChronoDuration::days(91))
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&admin)
    .await
    .expect("notifications");

    let (read, archived) = run_notification_gc(&pool, &cancel).await.expect("notif");
    assert_eq!(read, 1);
    assert_eq!(archived, 1);
    let (read_again, archived_again) = run_notification_gc(&pool, &cancel).await.expect("notif 2");
    assert_eq!(read_again, 0);
    assert_eq!(archived_again, 0);
    let leftover: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.notifications WHERE id = $1")
            .bind(fresh_read)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(leftover, 1);

    let older_event = Uuid::now_v7();
    let newer_event = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.events (id, verb, payload) VALUES ($1, 'task.updated', '{}'::jsonb)",
    )
    .bind(older_event)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.events (id, verb, payload) VALUES ($1, 'task.updated', '{}'::jsonb)",
    )
    .bind(newer_event)
    .execute(&admin)
    .await
    .unwrap();
    mark_processed(&pool, "notifications", older_event)
        .await
        .expect("mark old");
    mark_processed(&pool, "notifications", newer_event)
        .await
        .expect("mark new");
    sqlx::query(
        "UPDATE fvoci.processed_events SET processed_at = now() - interval '31 days' WHERE event_id = $1",
    )
    .bind(older_event)
    .execute(&admin)
    .await
    .unwrap();

    let processed = run_processed_gc(&pool, &cancel).await.expect("processed");
    assert_eq!(processed, 1);
    let processed_again = run_processed_gc(&pool, &cancel).await.expect("processed 2");
    assert_eq!(processed_again, 0);
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.processed_events WHERE event_id = $1 AND consumer = 'notifications'",
    )
    .bind(newer_event)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(kept, 1);

    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn digest_claim_sends_once_across_two_runners() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let sink = SmtpSink::spawn().await;
    let mailer = mailer_for(sink.port);

    sqlx::query(
        r#"
        INSERT INTO fvoci.notification_prefs (
            workspace_id, user_id, in_app, mail_immediate, mail_digest
        ) VALUES ($1, $2, true, true, true)
        ON CONFLICT (workspace_id, user_id) DO UPDATE
        SET mail_digest = true, last_digest_at = NULL
        "#,
    )
    .bind(workspace_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.notifications (
            id, workspace_id, user_id, event_id, verb, payload
        ) VALUES ($1, $2, $3, $4, 'task.updated', '{}'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(owner_id)
    .bind(Uuid::now_v7())
    .execute(&admin)
    .await
    .unwrap();

    let now = Utc::now();
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(
        send_due_digests(&pool, &mailer, now, &cancel),
        send_due_digests(&pool, &mailer, now, &cancel)
    );
    let sent = a.expect("a") + b.expect("b");
    assert_eq!(sent, 1, "exactly one runner should send");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while sink.count() < 1 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(sink.count(), 1);

    let again = send_due_digests(&pool, &mailer, now, &CancellationToken::new())
        .await
        .expect("idempotent");
    assert_eq!(again, 0);
    assert_eq!(sink.count(), 1);

    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn two_runners_only_one_claims_daily_sweep() {
    let harness = TestDb::bootstrap().await;
    let pool = app_pool(&harness).await;
    let held = JobClaim::try_claim(&pool, JOB_KEY_DAILY)
        .await
        .expect("claim")
        .expect("first claim wins");
    let other = JobClaim::try_claim(&pool, JOB_KEY_DAILY)
        .await
        .expect("second try");
    assert!(
        other.is_none(),
        "second process must not take the daily lock"
    );

    let (_root, storage) = temp_storage();
    let skipped = run_daily_sweep(
        &pool,
        &storage,
        &Mailer::disabled(),
        &CancellationToken::new(),
    )
    .await
    .expect("sweep while locked");
    assert!(skipped.is_none());

    held.release().await;
    let after = JobClaim::try_claim(&pool, JOB_KEY_DAILY)
        .await
        .expect("reclaim")
        .expect("lock returns after release");
    after.release().await;

    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn shutdown_drains_the_scheduler_loop() {
    let harness = TestDb::bootstrap().await;
    let pool = app_pool(&harness).await;
    let (_root, storage) = temp_storage();
    let handle = spawn_maintenance(
        MaintenanceSettings {
            tick: Duration::from_millis(20),
            interval: Duration::from_secs(3600),
        },
        pool.clone(),
        storage,
        Arc::new(Mailer::disabled()),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.request_shutdown();
    tokio::time::timeout(Duration::from_secs(2), handle.join())
        .await
        .expect("join within deadline")
        .expect("join ok");

    let cancel = CancellationToken::new();
    cancel.cancel();
    let (_root, storage) = temp_storage();
    let stats = run_workspace_purge(&pool, &storage, Utc::now(), &cancel)
        .await
        .expect("cancelled purge");
    assert_eq!(stats.purged, 0);

    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn two_workspace_purge_runners_converge() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();
    let doomed_id = create_team_workspace(app.clone(), &cookie, "Race", "race-team").await;
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{doomed_id}/documents"),
        Some(json!({"title": "첨부", "parentId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{wiki:?}");
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let key = Uuid::now_v7().to_string();
    write_object(&root, &key);
    insert_attachment_with_key(&admin, doomed_id, document_id, owner_id, &key).await;
    trash_workspace(app, &cookie, doomed_id, "race-team").await;
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = $2 WHERE id = $1")
        .bind(doomed_id)
        .bind(Utc::now() - ChronoDuration::days(31))
        .execute(&admin)
        .await
        .unwrap();

    let now = Utc::now();
    let cancel_a = CancellationToken::new();
    let cancel_b = CancellationToken::new();
    let (a, b) = tokio::join!(
        run_workspace_purge(&pool, &storage, now, &cancel_a),
        run_workspace_purge(&pool, &storage, now, &cancel_b)
    );
    let purged = a.expect("a").purged + b.expect("b").purged;
    assert_eq!(purged, 1);
    assert!(!object_exists(&root, &key));

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// A failed send hands the recipient's window back (source keeps lastDigestAt),
/// so the next sweep sends the digest with the original count.
#[tokio::test]
async fn failed_digest_send_is_retried_with_the_same_window() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let prev = Utc::now() - chrono::Duration::days(3);
    sqlx::query(
        r#"
        INSERT INTO fvoci.notification_prefs (
            workspace_id, user_id, in_app, mail_immediate, mail_digest, last_digest_at
        ) VALUES ($1, $2, true, true, true, $3)
        ON CONFLICT (workspace_id, user_id) DO UPDATE
        SET mail_digest = true, last_digest_at = $3
        "#,
    )
    .bind(workspace_id)
    .bind(owner_id)
    .bind(prev)
    .execute(&admin)
    .await
    .unwrap();
    for _ in 0..3 {
        sqlx::query(
            r#"
            INSERT INTO fvoci.notifications (
                id, workspace_id, user_id, event_id, verb, payload
            ) VALUES ($1, $2, $3, $4, 'task.updated', '{}'::jsonb)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(owner_id)
        .bind(Uuid::now_v7())
        .execute(&admin)
        .await
        .unwrap();
    }

    // SMTP down: nothing listens on this port.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = closed.local_addr().unwrap().port();
    drop(closed);
    let now = Utc::now();
    let sent = send_due_digests(
        &pool,
        &mailer_for(dead_port),
        now,
        &CancellationToken::new(),
    )
    .await
    .expect("sweep with failing smtp");
    assert_eq!(sent, 0);
    let restored: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT last_digest_at FROM fvoci.notification_prefs WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(owner_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        restored.map(|t| t.timestamp_micros()),
        Some(prev.timestamp_micros()),
        "failed send must hand the window back"
    );

    let sink = SmtpSink::spawn().await;
    let later = now + chrono::Duration::minutes(1);
    let sent = send_due_digests(
        &pool,
        &mailer_for(sink.port),
        later,
        &CancellationToken::new(),
    )
    .await
    .expect("retry sweep");
    assert_eq!(sent, 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while sink.count() < 1 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(sink.count(), 1);
    assert!(
        sink.last_text().contains('3'),
        "digest counts the 3 notifications of the original window: {}",
        sink.last_text()
    );

    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}
