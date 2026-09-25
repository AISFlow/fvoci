#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Image previews end to end: the real `fvoci-server --internal-image-preview`
//! child, the DB lease/journal/publish sequence, member and public-share
//! `variant=preview` downloads, hostile inputs, and `preview-html`.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use axum::http::StatusCode;
use fvoci_server::attachments::preview::{run_preview_helper, PreviewError, PreviewLimits};
use fvoci_server::attachments::{ObjectStorage, PreviewJobSettings};
use project_harness::{
    add_workspace_user, admin_pool, app_state, create_project, http_request, json_request,
    setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, 255])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// A valid PNG signature and IHDR declaring `width`×`height` followed by a
/// tiny IDAT: a decompression bomb by its header alone.
fn png_bomb(width: u32, height: u32) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = kind.to_vec();
        crc_input.extend_from_slice(data);
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(
        &mut out,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    chunk(&mut out, b"IEND", &[]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

struct Ctx {
    harness: TestDb,
    app: axum::Router,
    cookie: String,
    ws: Uuid,
    admin: PgPool,
    pool: PgPool,
    storage: ObjectStorage,
}

async fn ctx() -> Ctx {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    // The router's storage root is private to its AppState; rebuild the app
    // on one state so the job and the routes share storage.
    let state = app_state(&harness.app_url).await;
    let pool = state.auth.db.pool.clone();
    let storage = state.storage.clone();
    let app2 = fvoci_server::http::router(state, None);
    let _ = app;
    Ctx {
        harness,
        app: app2,
        cookie,
        ws,
        admin,
        pool,
        storage,
    }
}

impl Ctx {
    async fn done(self) {
        self.admin.close().await;
        self.harness.cleanup().await;
    }

    async fn wiki_doc(&self) -> String {
        let (status, body) = json_request(
            self.app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{}/documents", self.ws),
            Some(json!({"parentId": null, "title": "Doc"})),
            Some(&self.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().unwrap().to_string()
    }

    async fn upload(&self, create_path: &str, name: &str, bytes: &[u8]) -> Value {
        let (status, created) = json_request(
            self.app.clone(),
            "POST",
            create_path,
            Some(json!({"name": name, "sizeBytes": bytes.len()})),
            Some(&self.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["attachmentId"].as_str().unwrap().to_string();
        let part_size = created["partSizeBytes"].as_u64().unwrap() as usize;
        let mut parts = Vec::new();
        for part in created["parts"].as_array().unwrap() {
            let n = part["partNumber"].as_u64().unwrap() as usize;
            let start = (n - 1) * part_size;
            let end = (start + part_size).min(bytes.len());
            let (status, _, headers) = http_request(
                self.app.clone(),
                "PUT",
                part["url"].as_str().unwrap(),
                Some(bytes[start..end].to_vec()),
                Some("application/octet-stream"),
                Some(&self.cookie),
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
            parts.push(json!({"partNumber": n, "etag": etag}));
        }
        let (status, stored) = json_request(
            self.app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{}/attachments/{id}/complete", self.ws),
            Some(json!({"parts": parts})),
            Some(&self.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{stored}");
        stored
    }

    async fn run_job(&self) -> bool {
        let settings = PreviewJobSettings::new(helper());
        fvoci_server::attachments::process_one_preview(
            &settings,
            &self.pool,
            &self.storage,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    async fn preview_status(&self, id: &str) -> (String, Value) {
        sqlx::query_as("SELECT preview_status, variants FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn png_preview_is_rendered_published_and_served_with_etag_and_range() {
    let c = ctx().await;
    let doc = c.wiki_doc().await;
    let path = format!("/api/v1/workspaces/{}/documents/{doc}/uploads", c.ws);
    let stored = c.upload(&path, "wide.png", &png(3200, 800)).await;
    let id = stored["id"].as_str().unwrap().to_string();
    assert_eq!(stored["image"], true);
    assert!(stored["preview"].is_null());
    assert_eq!(c.preview_status(&id).await.0, "pending");
    let original_before: Value = stored.clone();

    assert!(c.run_job().await, "claimed");
    let (status, variants) = c.preview_status(&id).await;
    assert_eq!(status, "ok");
    let key = variants["preview"]["key"].as_str().unwrap().to_string();
    assert_eq!(variants["preview"]["width"], 1600);
    assert_eq!(variants["preview"]["height"], 400);
    let journal: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.attachment_object_cleanups")
        .fetch_one(&c.admin)
        .await
        .unwrap();
    assert_eq!(journal, 0, "publish removed the pre-write journal row");
    assert!(!c.run_job().await, "nothing left to claim");

    let (status, meta) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/attachments/{id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["preview"], json!({"width": 1600, "height": 400}));
    assert_eq!(meta["sizeBytes"], original_before["sizeBytes"]);

    let url = format!(
        "/api/v1/workspaces/{}/attachments/{id}/download?variant=preview",
        c.ws
    );
    let (status, _, headers) =
        http_request(c.app.clone(), "GET", &url, None, None, Some(&c.cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/webp");
    assert_eq!(headers["content-disposition"], "inline");
    assert_eq!(headers["content-security-policy"], "sandbox");
    assert_eq!(headers["cache-control"], "private, max-age=3600");
    let etag = headers["etag"].to_str().unwrap().to_string();
    assert_eq!(
        etag,
        fvoci_server::attachments::preview::preview_etag(
            &key,
            variants["preview"]["bytes"].as_i64().unwrap(),
            1600,
            400
        )
    );
    let weak = format!("W/{etag}");
    let (status, _, headers) = http_request(
        c.app.clone(),
        "GET",
        &url,
        None,
        None,
        Some(&c.cookie),
        &[("if-none-match", weak.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert_eq!(headers["etag"].to_str().unwrap(), etag);
    let (status, _, headers) = http_request(
        c.app.clone(),
        "GET",
        &url,
        None,
        None,
        Some(&c.cookie),
        &[("range", "bytes=0-3")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(headers["content-length"], "4");

    // The stored bytes really are a 1600×400 WebP.
    let bytes = c
        .storage
        .read_range(&key, 0, variants["preview"]["bytes"].as_u64().unwrap() - 1)
        .await
        .unwrap();
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1600, 400));

    // A viewer without access to the document gets nothing.
    let guest = add_workspace_user(&c.admin, c.ws, "guest", "guest").await;
    let (status, _, _) = http_request(
        c.app.clone(),
        "GET",
        &url,
        None,
        None,
        Some(&guest.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Public share of the document serves the same preview (no Range).
    let (status, body) = json_request(
        c.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/documents/{doc}/share-links", c.ws),
        Some(json!({})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let (status, meta) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/share/{token}/attachments/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["preview"], json!({"width": 1600, "height": 400}));
    let (status, _, headers) = http_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/share/{token}/attachments/{id}/download?variant=preview"),
        None,
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/webp");
    assert_eq!(headers["etag"].to_str().unwrap(), etag);
    c.done().await;
}

#[tokio::test]
async fn task_attachment_gets_a_preview_and_delete_reclaims_both_objects() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "PRE", "workspace").await;
    let (status, task) = json_request(
        c.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{}/tasks",
            c.ws,
            project["id"].as_str().unwrap()
        ),
        Some(json!({"title": "T"})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let stored = c
        .upload(
            &format!(
                "/api/v1/workspaces/{}/tasks/{}/uploads",
                c.ws,
                task["id"].as_str().unwrap()
            ),
            "small.png",
            &png(40, 30),
        )
        .await;
    let id = stored["id"].as_str().unwrap().to_string();
    assert!(c.run_job().await);
    let (status, variants) = c.preview_status(&id).await;
    assert_eq!(status, "ok");
    assert_eq!(
        variants["preview"]["width"], 40,
        "small images keep their size"
    );
    let preview_key = variants["preview"]["key"].as_str().unwrap().to_string();
    let original_key: String =
        sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_one(&c.admin)
            .await
            .unwrap();
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c.storage.head(&preview_key).await.unwrap(), None);
    assert_eq!(c.storage.head(&original_key).await.unwrap(), None);
    let journal: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.attachment_object_cleanups")
        .fetch_one(&c.admin)
        .await
        .unwrap();
    assert_eq!(journal, 0);
    c.done().await;
}

#[tokio::test]
async fn hostile_images_fail_without_a_preview() {
    let c = ctx().await;
    let doc = c.wiki_doc().await;
    let path = format!("/api/v1/workspaces/{}/documents/{doc}/uploads", c.ws);
    // 100k × 100k declared by the header: refused before any pixel buffer.
    let bomb = c
        .upload(&path, "bomb.png", &png_bomb(100_000, 100_000))
        .await;
    let mut truncated = png(256, 256);
    truncated.truncate(truncated.len() / 3);
    let cut = c.upload(&path, "cut.png", &truncated).await;
    // Sniffed as an image type outside the preview codec set.
    let mut ico = vec![0x00, 0x00, 0x01, 0x00, 0x01, 0x00];
    ico.extend_from_slice(&[0u8; 64]);
    let other = c.upload(&path, "icon.ico", &ico).await;
    for att in [&bomb, &cut] {
        assert!(c.run_job().await);
        let (status, variants) = c.preview_status(att["id"].as_str().unwrap()).await;
        assert_eq!(status, "failed", "{}", att["name"]);
        assert!(variants.get("preview").is_none());
    }
    let (status, _) = c.preview_status(other["id"].as_str().unwrap()).await;
    assert_ne!(status, "pending", "non-preview formats never queue");
    assert!(!c.run_job().await);
    let (status, _, _) = http_request(
        c.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/attachments/{}/download?variant=preview",
            c.ws,
            bomb["id"].as_str().unwrap()
        ),
        None,
        None,
        Some(&c.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let journal: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.attachment_object_cleanups")
        .fetch_one(&c.admin)
        .await
        .unwrap();
    assert_eq!(journal, 0, "no object was written for a failed preview");
    c.done().await;
}

#[tokio::test]
async fn preview_child_is_bounded_by_rlimit_timeout_and_pixel_limit() {
    let source = png(3000, 3000);
    // Address space too small to hold the decoded frame: the child dies.
    let tight = PreviewLimits {
        child_address_space: 96 * 1024 * 1024,
        ..PreviewLimits::default()
    };
    let err = run_preview_helper(&helper(), source.clone(), &tight)
        .await
        .unwrap_err();
    // Allocation beyond RLIMIT_AS aborts the child; the parent sees a kill.
    assert!(matches!(err, PreviewError::ResourceLimit(_)), "{err:?}");
    let slow = PreviewLimits {
        timeout: Duration::from_millis(1),
        ..PreviewLimits::default()
    };
    let err = run_preview_helper(&helper(), source.clone(), &slow)
        .await
        .unwrap_err();
    assert!(matches!(err, PreviewError::ResourceLimit(_)), "{err:?}");
    let small = PreviewLimits {
        input_pixels: 1_000_000,
        ..PreviewLimits::default()
    };
    let err = run_preview_helper(&helper(), source.clone(), &small)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, PreviewError::Rejected(msg) if msg.contains("pixel")),
        "{err:?}"
    );
    let ok = run_preview_helper(&helper(), source, &PreviewLimits::default())
        .await
        .unwrap();
    assert_eq!((ok.width, ok.height), (1600, 1600));
}

#[tokio::test]
async fn lost_lease_keeps_the_written_key_journaled_for_reclaim() {
    let c = ctx().await;
    let doc = c.wiki_doc().await;
    let path = format!("/api/v1/workspaces/{}/documents/{doc}/uploads", c.ws);
    let stored = c.upload(&path, "a.png", &png(20, 20)).await;
    let id = stored["id"].as_str().unwrap().to_string();
    let claim = fvoci_server::db::attachment_preview::claim_preview(&c.pool)
        .await
        .unwrap()
        .unwrap();
    let key = Uuid::now_v7().to_string();
    let journal = fvoci_server::db::attachment_preview::journal_preview_key(&c.pool, &claim, &key)
        .await
        .unwrap();
    c.storage
        .put_bytes(&key, b"RIFFfake".to_vec())
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.attachments SET preview_lease_expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .execute(&c.admin)
        .await
        .unwrap();
    let published = fvoci_server::db::attachment_preview::publish_preview(
        &c.pool, &claim, journal, &key, 20, 20, 8,
    )
    .await
    .unwrap();
    assert!(!published);
    let (status, variants) = c.preview_status(&id).await;
    assert_eq!(status, "pending");
    assert!(variants.get("preview").is_none());
    // The journal row becomes due after the lease window; force it due.
    sqlx::query("UPDATE fvoci.attachment_object_cleanups SET due_at = now() - interval '1 second'")
        .execute(&c.admin)
        .await
        .unwrap();
    let stats =
        fvoci_server::db::attachments::reclaim_attachment_objects(&c.pool, &c.storage, None, 10)
            .await
            .unwrap();
    assert_eq!(stats.reclaimed, 1);
    assert_eq!(c.storage.head(&key).await.unwrap(), None);
    // The next claim renders and publishes normally.
    assert!(c.run_job().await);
    assert_eq!(c.preview_status(&id).await.0, "ok");
    c.done().await;
}

#[tokio::test]
async fn preview_html_follows_the_mode_setting_and_escapes_text() {
    let c = ctx().await;
    let doc = c.wiki_doc().await;
    let path = format!("/api/v1/workspaces/{}/documents/{doc}/uploads", c.ws);
    let hwp = c.upload(&path, "memo.hwpx", b"PK\x03\x04hwpx").await;
    let hwp_id = hwp["id"].as_str().unwrap().to_string();
    let docx = c.upload(&path, "memo.docx", b"PK\x03\x04docx").await;
    let docx_id = docx["id"].as_str().unwrap().to_string();
    let html_path = |id: &str| format!("/api/v1/workspaces/{}/attachments/{id}/preview-html", c.ws);

    // auto (default): office only; nothing extracted yet → 413.
    let (status, _) = json_request(
        c.app.clone(),
        "GET",
        &html_path(&hwp_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = json_request(
        c.app.clone(),
        "GET",
        &html_path(&docx_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert_eq!(body["code"], "preview_not_available");

    sqlx::query(
        "INSERT INTO fvoci.instance_settings (key, value) VALUES ('attachmentPreview', '{\"mode\":\"server\"}')",
    )
    .execute(&c.admin)
    .await
    .unwrap();
    sqlx::query("UPDATE fvoci.attachments SET extract_text = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&hwp_id).unwrap())
        .bind("<script>alert('x')</script> & 한글")
        .execute(&c.admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        c.app.clone(),
        "GET",
        &html_path(&hwp_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["html"],
        "<pre>&lt;script&gt;alert(&#x27;x&#x27;)&lt;/script&gt; &amp; 한글</pre>"
    );

    sqlx::query("UPDATE fvoci.instance_settings SET value = '{\"mode\":\"client\"}' WHERE key = 'attachmentPreview'")
        .execute(&c.admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        c.app.clone(),
        "GET",
        &html_path(&hwp_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    c.done().await;
}

/// Review should-fix 3: images stored before 030 are queued for a thumbnail.
#[tokio::test]
async fn upgrade_to_030_queues_previews_for_stored_images() {
    let db = TestDb::bootstrap_through(29).await;
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db.admin_url)
        .await
        .unwrap();
    let (user_id, workspace_id, document_id) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Up')")
        .bind(user_id)
        .bind(format!("up-{}@example.com", user_id.simple()))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'ws')")
        .bind(workspace_id)
        .bind(format!("p{}", &user_id.simple().to_string()[..16]))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number,
            status, schema_version, text, chosung, created_by, content_json, kind
        ) VALUES ($1, $2, 'Doc', $3, NULL, 'V', NULL, 1, 'published', 1, 'Doc', '', $4,
            '{"type":"doc","content":[]}'::jsonb, 'wiki')"#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(document_id.simple().to_string())
    .bind(user_id)
    .execute(&admin)
    .await
    .expect("document");
    let mut ids = Vec::new();
    for (status, mime, image) in [
        ("stored", "image/png", true),
        ("stored", "image/webp", true),
        ("stored", "application/x-hwp", false),
        ("stored", "image/svg+xml", true),
        ("uploading", "image/png", true),
    ] {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO fvoci.attachments (
                id, workspace_id, document_id, uploader_id, status, name, mime,
                size_bytes, reserved_size_bytes, storage_key, image, scan_status,
                extract_status, completed_at
            ) VALUES ($1, $2, $3, $4, $5, 'f', $6, CASE WHEN $5 = 'stored' THEN 16 END, 16, $7, $8, 'skipped', 'skipped',
                CASE WHEN $5 = 'stored' THEN now() END)"#,
        )
        .bind(id)
        .bind(workspace_id)
        .bind(document_id)
        .bind(user_id)
        .bind(status)
        .bind(mime)
        .bind(format!("attachments/{workspace_id}/{id}"))
        .bind(image)
        .execute(&admin)
        .await
        .expect("attachment");
        ids.push(id);
    }
    fvoci_server::db::migrate::run_migrations_through(&db.admin_url, 30)
        .await
        .expect("migrate to 30");
    let mut statuses = Vec::new();
    for id in &ids {
        let status: String =
            sqlx::query_scalar("SELECT preview_status FROM fvoci.attachments WHERE id = $1")
                .bind(id)
                .fetch_one(&admin)
                .await
                .unwrap();
        statuses.push(status);
    }
    assert_eq!(
        statuses,
        vec!["pending", "pending", "skipped", "skipped", "skipped"],
        "only stored images of supported types are queued"
    );
    admin.close().await;
    db.cleanup().await;
}
