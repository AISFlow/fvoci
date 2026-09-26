#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::attachments::ObjectStorage;
use fvoci_server::auth::token::hash_token;
use fvoci_server::db::magic::issue_password_reset_token;
use fvoci_server::db::outbox::mark_processed;
use fvoci_server::jobs::{
    run_daily_sweep, run_document_trash_purge, run_document_trash_purge_with, run_ics_token_gc,
    run_magic_token_gc, run_notification_gc, run_processed_gc, run_stale_upload_gc,
    run_stale_upload_sweep, run_workspace_purge, spawn_maintenance, DocumentPurgeLimits, JobClaim,
    MaintenanceSettings, JOB_KEY_DAILY, JOB_KEY_UPLOADS,
};
use fvoci_server::mail::{send_due_digests, Mailer, SmtpConfig};
use project_harness::{admin_pool, app_pool, create_project, json_request, setup_session, TestDb};
use serde_json::json;
use sqlx::{Connection, PgPool};
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

fn temp_storage() -> (PathBuf, ObjectStorage) {
    let root = std::env::temp_dir().join(format!("fvoci-jobs-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).expect("storage root");
    (root.clone(), ObjectStorage::local(root))
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
            ..MaintenanceSettings::default()
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

async fn insert_stale_upload(
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
            storage_key, created_at
        ) VALUES ($1, $2, $3, $4, 'uploading', 'stale.bin', 4, $5, now() - interval '25 hours')
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(uploader_id)
    .bind(storage_key)
    .execute(admin)
    .await
    .expect("insert stale upload");
    attachment_id
}

async fn attachment_exists(admin: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.attachments WHERE id = $1")
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap()
        == 1
}

/// Abandoned-upload cleanup runs inside the one maintenance scheduler, under
/// its own claim key and cadence, and drains with it on shutdown.
#[tokio::test]
async fn scheduler_runs_stale_upload_gc_under_its_own_claim() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"title": "upload gc", "parentId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{wiki:?}");
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let key = Uuid::now_v7().to_string();
    let staging = root.join("tmp").join(&key);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("1"), b"part").unwrap();
    let stale = insert_stale_upload(&admin, workspace_id, document_id, owner_id, &key).await;

    // Another process holds the upload cleanup claim: this runner skips.
    let held = JobClaim::try_claim(&pool, JOB_KEY_UPLOADS)
        .await
        .unwrap()
        .expect("claim");
    let ttl = Duration::from_secs(24 * 60 * 60);
    let skipped = run_stale_upload_sweep(&pool, &storage, ttl, None, &CancellationToken::new())
        .await
        .unwrap();
    assert!(skipped.is_none());
    assert!(attachment_exists(&admin, stale).await);
    // The daily sweep's claim is separate and unaffected.
    let daily = JobClaim::try_claim(&pool, JOB_KEY_DAILY)
        .await
        .unwrap()
        .expect("daily claim is independent");
    daily.release().await;
    held.release().await;

    let handle = spawn_maintenance(
        MaintenanceSettings {
            tick: Duration::from_millis(20),
            interval: Duration::from_secs(3600),
            upload_gc_interval: Duration::from_secs(3600),
            upload_incomplete_ttl: ttl,
        },
        pool.clone(),
        storage.clone(),
        Arc::new(Mailer::disabled()),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while attachment_exists(&admin, stale).await {
        assert!(
            std::time::Instant::now() < deadline,
            "scheduler did not purge the stale upload"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!staging.exists(), "staged parts must be removed");
    handle.request_shutdown();
    tokio::time::timeout(Duration::from_secs(2), handle.join())
        .await
        .expect("join within deadline")
        .expect("join ok");

    // Idempotent once drained.
    let again = run_stale_upload_sweep(&pool, &storage, ttl, None, &CancellationToken::new())
        .await
        .unwrap()
        .expect("claim free after shutdown");
    assert_eq!(again.purged, 0);

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// Review D4: a row that is skipped on every run (its session lock is held)
/// must not keep later rows, in any workspace, out of reach. Each run resumes
/// after the previous batch in global `(created_at, id)` order and wraps.
#[tokio::test]
async fn upload_gc_cursor_does_not_starve_rows_behind_a_stuck_one() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();
    let other_ws = create_team_workspace(app.clone(), &cookie, "Other", "gc-other").await;
    let mut documents = Vec::new();
    for ws in [workspace_id, other_ws] {
        let (status, doc) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/documents"),
            Some(json!({"title": "gc cursor", "parentId": null})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED, "{doc:?}");
        documents.push(Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap());
    }
    // Oldest row A (stuck), then B and C in the other workspace.
    let mut ids = Vec::new();
    for (ws, doc, hours) in [
        (workspace_id, documents[0], 30),
        (other_ws, documents[1], 28),
        (other_ws, documents[1], 26),
    ] {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.attachments (
                id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
                storage_key, created_at
            ) VALUES ($1, $2, $3, $4, 'uploading', 'stale.bin', 4, $5,
                      now() - make_interval(hours => $6))
            "#,
        )
        .bind(id)
        .bind(ws)
        .bind(doc)
        .bind(owner_id)
        .bind(Uuid::now_v7().to_string())
        .bind(hours)
        .execute(&admin)
        .await
        .unwrap();
        ids.push(id);
    }
    // Another session holds A's upload session lock for the whole test.
    let mut holder = sqlx::PgConnection::connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(fvoci_server::attachments::ATTACHMENT_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(ids[0]))
        .execute(&mut holder)
        .await
        .unwrap();

    let cutoff = Utc::now() - ChronoDuration::hours(24);
    let cancel = CancellationToken::new();
    let run = |after| {
        let pool = pool.clone();
        let storage = storage.clone();
        let cancel = cancel.clone();
        async move {
            run_stale_upload_gc(&pool, &storage, cutoff, after, 1, &cancel)
                .await
                .unwrap()
        }
    };

    // Without a cursor every run would pick the stuck row again.
    let first = run(None).await;
    assert_eq!((first.claimed, first.purged), (1, 0));
    assert_eq!(first.resume_after.map(|c| c.1), Some(ids[0]));
    let second = run(first.resume_after).await;
    assert_eq!(second.purged, 1);
    assert!(!attachment_exists(&admin, ids[1]).await);
    let third = run(second.resume_after).await;
    assert_eq!(third.purged, 1);
    assert!(!attachment_exists(&admin, ids[2]).await);
    // Past the end: nothing left, and the cursor wraps to the start.
    let fourth = run(third.resume_after).await;
    assert_eq!(fourth.claimed, 0);
    assert_eq!(fourth.resume_after, None);
    let wrapped = run(fourth.resume_after).await;
    assert_eq!((wrapped.claimed, wrapped.purged), (1, 0));
    assert!(attachment_exists(&admin, ids[0]).await);

    // Once the lock is gone the stuck row is purged too.
    holder.close().await.unwrap();
    let freed = run(None).await;
    assert_eq!(freed.purged, 1);
    assert!(!attachment_exists(&admin, ids[0]).await);

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

async fn create_wiki_doc(app: &axum::Router, cookie: &str, ws: Uuid, parent: Option<Uuid>) -> Uuid {
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"title": "휴지통 문서", "parentId": parent})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{doc:?}");
    Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap()
}

async fn trash_wiki(app: &axum::Router, cookie: &str, ws: Uuid, id: Uuid) {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{id}/trash"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body:?}");
}

async fn age_trash(admin: &PgPool, ids: &[Uuid], hours: i32) {
    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = now() - make_interval(hours => $2) WHERE id = ANY($1)",
    )
    .bind(ids)
    .bind(hours)
    .execute(admin)
    .await
    .unwrap();
}

async fn document_exists(admin: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.documents WHERE id = $1")
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap()
        > 0
}

const DAY: i32 = 24;

async fn restore_wiki_status(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    id: Uuid,
) -> axum::http::StatusCode {
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{id}/restore"),
        None,
        Some(cookie),
    )
    .await
    .0
}

async fn purged_event_channels(admin: &PgPool, ws: Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT channel FROM fvoci.events WHERE workspace_id = $1 AND verb = 'document.purged'",
    )
    .bind(ws)
    .fetch_all(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn document_trash_purge_removes_storage_then_rows_after_retention() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let parent = create_wiki_doc(&app, &cookie, ws, None).await;
    let child = create_wiki_doc(&app, &cookie, ws, Some(parent)).await;
    let fresh = create_wiki_doc(&app, &cookie, ws, None).await;
    let restored = create_wiki_doc(&app, &cookie, ws, None).await;
    let margin = create_wiki_doc(&app, &cookie, ws, None).await;
    let parent_key = Uuid::now_v7().to_string();
    let child_key = Uuid::now_v7().to_string();
    write_object(&root, &parent_key);
    write_object(&root, &child_key);
    let parent_att = insert_attachment_with_key(&admin, ws, parent, owner_id, &parent_key).await;
    insert_attachment_with_key(&admin, ws, child, owner_id, &child_key).await;
    sqlx::query(
        r#"
        INSERT INTO fvoci.revisions (id, workspace_id, target_kind, target_id, y_snapshot,
                                     content_json, text, reason)
        VALUES ($1, $2, 'document', $3, '\x00', '{}'::jsonb, '', 'manual')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(parent)
    .execute(&admin)
    .await
    .unwrap();
    trash_wiki(&app, &cookie, ws, parent).await;
    trash_wiki(&app, &cookie, ws, fresh).await;
    trash_wiki(&app, &cookie, ws, restored).await;
    trash_wiki(&app, &cookie, ws, margin).await;
    age_trash(&admin, &[parent, child, restored], 32 * DAY).await;
    // Past the restore retention (30 days) but inside the purge margin day.
    age_trash(&admin, &[margin], 30 * DAY + 12).await;
    sqlx::query("UPDATE fvoci.documents SET deleted_at = NULL WHERE id = $1")
        .bind(restored)
        .execute(&admin)
        .await
        .unwrap();

    let cancel = CancellationToken::new();
    let stats = run_document_trash_purge(&pool, &storage, &cancel)
        .await
        .expect("purge");
    // Deepest first: the child goes, then its parent in the same run.
    assert_eq!(stats.purged, 2, "{stats:?}");
    assert_eq!(stats.storage_deleted, 2);
    assert!(!document_exists(&admin, parent).await);
    assert!(!document_exists(&admin, child).await);
    assert!(!object_exists(&root, &parent_key));
    assert!(!object_exists(&root, &child_key));
    assert!(!attachment_exists(&admin, parent_att).await);
    let revisions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.revisions WHERE target_id = $1")
            .bind(parent)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(revisions, 0);
    assert!(document_exists(&admin, fresh).await, "inside retention");
    assert!(
        document_exists(&admin, restored).await,
        "restored rows are never purged"
    );
    assert!(
        document_exists(&admin, margin).await,
        "the purge waits a margin day past the restore retention"
    );
    assert_eq!(
        restore_wiki_status(&app, &cookie, ws, margin).await,
        axum::http::StatusCode::NOT_FOUND,
        "restore already refuses a row past the retention"
    );
    assert_eq!(
        restore_wiki_status(&app, &cookie, ws, fresh).await,
        axum::http::StatusCode::OK
    );
    assert_eq!(
        purged_event_channels(&admin, ws).await,
        vec!["system".to_string(), "system".to_string()]
    );

    let again = run_document_trash_purge(&pool, &storage, &cancel)
        .await
        .expect("idempotent");
    assert_eq!(again.purged, 0);

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// Storage first: a storage failure keeps every row (including the document)
/// for the next sweep, even after an earlier key of the same document was
/// already deleted; restore refuses that row meanwhile. An object already gone
/// (crash after storage, before the DB delete) counts as deleted.
#[tokio::test]
async fn document_trash_purge_keeps_rows_when_storage_fails() {
    use std::os::unix::fs::PermissionsExt;

    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let doc = create_wiki_doc(&app, &cookie, ws, None).await;
    let first_key = Uuid::now_v7().to_string();
    let key = Uuid::now_v7().to_string();
    write_object(&root, &first_key);
    write_object(&root, &key);
    let first_att = insert_attachment_with_key(&admin, ws, doc, owner_id, &first_key).await;
    let att = insert_attachment_with_key(&admin, ws, doc, owner_id, &key).await;
    let crashed = create_wiki_doc(&app, &cookie, ws, None).await;
    let missing_key = Uuid::now_v7().to_string();
    let crashed_att = insert_attachment_with_key(&admin, ws, crashed, owner_id, &missing_key).await;
    trash_wiki(&app, &cookie, ws, doc).await;
    trash_wiki(&app, &cookie, ws, crashed).await;
    age_trash(&admin, &[doc, crashed], 32 * DAY).await;

    // The second object's directory cannot be removed: storage error after the
    // first key of the same document was deleted.
    let object_dir = root.join("objects").join(&key);
    std::fs::set_permissions(&object_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let cancel = CancellationToken::new();
    let failed = run_document_trash_purge(&pool, &storage, &cancel)
        .await
        .expect("sweep");
    std::fs::set_permissions(&object_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(failed.failed, 1, "{failed:?}");
    assert_eq!(
        failed.purged, 1,
        "the document whose object is already gone is purged"
    );
    assert!(!object_exists(&root, &first_key), "partial storage delete");
    assert!(document_exists(&admin, doc).await);
    assert!(attachment_exists(&admin, first_att).await);
    assert!(attachment_exists(&admin, att).await);
    assert_eq!(
        restore_wiki_status(&app, &cookie, ws, doc).await,
        axum::http::StatusCode::NOT_FOUND,
        "a partially purged document is never restored"
    );
    assert!(!document_exists(&admin, crashed).await);
    assert!(!attachment_exists(&admin, crashed_att).await);

    let retry = run_document_trash_purge(&pool, &storage, &cancel)
        .await
        .expect("retry");
    assert_eq!(retry.purged, 1, "{retry:?}");
    assert_eq!(retry.storage_deleted, 2, "the missing first key counts");
    assert!(!document_exists(&admin, doc).await);
    assert!(!object_exists(&root, &key));

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// The storage step holds no workspace lock: while the purge is paused before
/// storage, other tree writes of the workspace proceed, and a restore of the
/// document being purged is refused at once (expired), so nothing interleaves.
#[tokio::test]
async fn document_trash_purge_storage_step_holds_no_tree_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let doc = create_wiki_doc(&app, &cookie, ws, None).await;
    let key = Uuid::now_v7().to_string();
    write_object(&root, &key);
    insert_attachment_with_key(&admin, ws, doc, owner_id, &key).await;
    trash_wiki(&app, &cookie, ws, doc).await;
    age_trash(&admin, &[doc], 32 * DAY).await;

    let (reached, proceed) = fvoci_server::db::document_purge::test_hooks::arm_before_storage(doc);
    let purge = tokio::spawn({
        let pool = pool.clone();
        let storage = storage.clone();
        async move {
            run_document_trash_purge(&pool, &storage, &CancellationToken::new())
                .await
                .unwrap()
        }
    });
    reached.await.expect("purge reached storage step");
    let held: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM pg_locks
        WHERE locktype = 'advisory' AND granted
          AND database = (SELECT oid FROM pg_database WHERE datname = current_database())
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(held, 0, "no advisory (tree) lock during the storage step");
    // A tree write and the restore both finish while the purge is paused.
    let sibling = create_wiki_doc(&app, &cookie, ws, None).await;
    assert!(document_exists(&admin, sibling).await);
    assert_eq!(
        restore_wiki_status(&app, &cookie, ws, doc).await,
        axum::http::StatusCode::NOT_FOUND
    );
    assert!(!purge.is_finished());
    proceed.send(()).unwrap();
    let stats = purge.await.unwrap();
    assert_eq!(stats.purged, 1, "{stats:?}");
    assert!(!document_exists(&admin, doc).await);
    assert!(!object_exists(&root, &key));

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// A deleted project's documents (root included) are purged deepest first with
/// their attachments and cascading rows; the project row stays (source keeps
/// it too) and can no longer be restored.
#[tokio::test]
async fn document_trash_purge_removes_deleted_project_subtree() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let project = create_project(app.clone(), &cookie, ws, "GONE", "workspace").await;
    let pid = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let root_doc = Uuid::parse_str(project["rootDocumentId"].as_str().unwrap()).unwrap();
    let project_url = format!("/api/v1/workspaces/{ws}/projects/{pid}");
    let (status, child) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/documents"),
        Some(json!({"parentId": root_doc.to_string(), "title": "하위"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{child:?}");
    let child = Uuid::parse_str(child["id"].as_str().unwrap()).unwrap();
    let key = Uuid::now_v7().to_string();
    write_object(&root, &key);
    let att = insert_attachment_with_key(&admin, ws, child, owner_id, &key).await;
    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/documents/{child}/comments"),
        Some(json!({"body": "지워질 댓글"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{comment:?}");
    let (status, _) = json_request(app.clone(), "DELETE", &project_url, None, Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    age_trash(&admin, &[root_doc, child], 32 * DAY).await;
    sqlx::query("UPDATE fvoci.projects SET deleted_at = now() - interval '32 days' WHERE id = $1")
        .bind(pid)
        .execute(&admin)
        .await
        .unwrap();

    let stats = run_document_trash_purge(&pool, &storage, &CancellationToken::new())
        .await
        .expect("purge");
    assert_eq!(stats.purged, 2, "{stats:?}");
    assert_eq!(stats.storage_deleted, 1);
    assert!(!document_exists(&admin, child).await);
    assert!(!document_exists(&admin, root_doc).await);
    assert!(!attachment_exists(&admin, att).await);
    assert!(!object_exists(&root, &key));
    let comments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.comments WHERE document_id = $1")
            .bind(child)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(comments, 0, "comments cascade with the document");
    let project_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.projects WHERE id = $1")
        .bind(pid)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(project_rows, 1);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{project_url}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

/// Documents that keep failing never stall the rest: each run excludes ids it
/// already examined and keeps listing until the workspace has nothing left.
/// With no budget left the sweep stops without touching anything.
#[tokio::test]
async fn document_trash_purge_advances_past_failing_documents() {
    use std::os::unix::fs::PermissionsExt;

    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (root, storage) = temp_storage();

    let mut failing = Vec::new();
    let mut bad_dirs = Vec::new();
    for _ in 0..2 {
        let doc = create_wiki_doc(&app, &cookie, ws, None).await;
        let key = Uuid::now_v7().to_string();
        write_object(&root, &key);
        insert_attachment_with_key(&admin, ws, doc, owner_id, &key).await;
        trash_wiki(&app, &cookie, ws, doc).await;
        failing.push(doc);
        bad_dirs.push(root.join("objects").join(&key));
    }
    let good = create_wiki_doc(&app, &cookie, ws, None).await;
    trash_wiki(&app, &cookie, ws, good).await;
    // The failing rows sort first (older trash stamp, same depth).
    age_trash(&admin, &failing, 40 * DAY).await;
    age_trash(&admin, &[good], 32 * DAY).await;

    let idle = run_document_trash_purge_with(
        &pool,
        &storage,
        DocumentPurgeLimits {
            batch: 1,
            budget: Duration::ZERO,
        },
        &CancellationToken::new(),
    )
    .await
    .expect("no budget");
    assert_eq!(idle.purged + idle.failed + idle.skipped, 0, "{idle:?}");

    for dir in &bad_dirs {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let stats = run_document_trash_purge_with(
        &pool,
        &storage,
        DocumentPurgeLimits {
            batch: 1,
            ..DocumentPurgeLimits::default()
        },
        &CancellationToken::new(),
    )
    .await
    .expect("sweep");
    for dir in &bad_dirs {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(stats.failed, 2, "{stats:?}");
    assert_eq!(stats.purged, 1, "{stats:?}");
    assert!(!document_exists(&admin, good).await);
    for doc in &failing {
        assert!(document_exists(&admin, *doc).await);
    }

    let _ = std::fs::remove_dir_all(&root);
    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}
