#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/libsql_finish_fixture.rs"]
mod libsql_fixture;
#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, hold_membership_user_lock,
    http_request, json_request, session_id_for_user, setup_session, wait_for_advisory_blocked_by,
    TestDb,
};
use serde_json::{json, Value};
use uuid::Uuid;

#[tokio::test]
async fn sqlite_group_creation_rechecks_membership_and_commits() {
    use fvoci_server::auth::password::Keyring;
    use fvoci_server::auth::AuthService;
    use fvoci_server::db::{backend::Backend, migrate, pool, Db};
    use fvoci_server::http::{router, state::AppState};
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("fvoci-membership-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("db.sqlite");
    migrate::run_sqlite_migrations(&path).await.unwrap();
    let sqlite = pool::connect_sqlite_app(&path, 3).await.unwrap();
    let auth = Arc::new(AuthService {
        db: Db::from_backend(Backend::Sqlite(sqlite.clone())),
        password_keys: Keyring::parse(
            r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
            "test",
        )
        .unwrap(),
    });
    let state = AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::Off,
        native_engine: None,
        auth,
        branding_name: "FVOCI".into(),
        public_origin: "http://localhost".into(),
        cookie_secure: false,
        rate_limiter: fvoci_server::http::rate_limit::RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(root.join("storage")).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        search_embedder: None,
        streams: AppState::fresh_streams(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
        quota: Default::default(),
        markdown: None,
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
    };
    let app = router(state, None);
    let (status, _, headers) = http_request(app.clone(), "POST", "/api/v1/setup",
        Some(json!({"email":"group@example.invalid","password":"Synthetic-Group-2026!","givenName":"Owner","workspaceSlug":"groups","workspaceName":"Groups"}).to_string().into_bytes()),
        Some("application/json"), None, &[]).await;
    assert_eq!(status, StatusCode::CREATED);
    let cookie = headers
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string();
    let ws_bytes: Vec<u8> = sqlx::query_scalar("SELECT id FROM workspaces WHERE slug='groups'")
        .fetch_one(&sqlite)
        .await
        .unwrap();
    let ws = Uuid::from_slice(&ws_bytes).unwrap();
    let (status, group) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/groups"),
        Some(json!({"name":"Team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let reopened = pool::connect_sqlite_app(&path, 1).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM groups WHERE name='Team'")
            .fetch_one(&reopened)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1")
        .bind(ws.as_bytes().as_slice())
        .execute(&sqlite)
        .await
        .unwrap();
    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/groups"),
        Some(json!({"name":"Denied"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM groups")
            .fetch_one(&reopened)
            .await
            .unwrap(),
        1
    );
    reopened.close().await;
    sqlite.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

async fn bearer(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: &str,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    let bytes = body.map(|value| value.to_string().into_bytes());
    let (status, json, _) = http_request(
        app,
        method,
        path,
        bytes,
        Some("application/json"),
        None,
        &[("authorization", &auth)],
    )
    .await;
    (status, json)
}

/// Discovery is a separate, authorized bulk read. The legacy wiki tree must
/// not acquire project rows or infer guest access to a granted child's parent.
#[tokio::test]
async fn wiki_discovery_bulk_grants_tags_and_default_tree_boundary() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "discovery-member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "discovery-guest").await;
    let public = create_project(app.clone(), &cookie, ws, "PUB", "workspace").await;
    let private = create_project(app.clone(), &cookie, ws, "GRANT", "private").await;
    let hidden = create_project(app.clone(), &member.cookie, ws, "HIDDEN", "private").await;
    let public_id: Uuid = public["id"].as_str().unwrap().parse().unwrap();
    let private_id: Uuid = private["id"].as_str().unwrap().parse().unwrap();
    let public_root = public["rootDocumentId"].as_str().unwrap();
    let private_root = private["rootDocumentId"].as_str().unwrap();
    let hidden_root = hidden["rootDocumentId"].as_str().unwrap();
    let project_child = Uuid::now_v7();
    project_harness::insert_project_document(&admin, ws, public_id, project_child, owner_id, 10)
        .await;
    sqlx::query("UPDATE fvoci.documents SET parent_id=$2, status='draft' WHERE id=$1")
        .bind(project_child)
        .bind(public_root.parse::<Uuid>().unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let mut wiki = Vec::new();
    for parent in [None, None, Some(0)] {
        let (status, doc) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/documents"),
            Some(json!({"commandId": uuid::Uuid::now_v7(), "title": "discovery wiki", "parentId": parent.map(|i| &wiki[i])})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{doc}");
        wiki.push(doc["id"].as_str().unwrap().to_string());
    }
    let child_id: Uuid = wiki[2].parse().unwrap();
    // Multiple overlapping groups must never duplicate discovery rows.
    for i in 0..2 {
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.groups (id, workspace_id, name) VALUES ($1, $2, $3)")
            .bind(group)
            .bind(ws)
            .bind(format!("discovery-{i}"))
            .execute(&admin)
            .await
            .unwrap();
        for user in [member.user_id, guest.user_id] {
            sqlx::query("INSERT INTO fvoci.group_members (workspace_id, group_id, user_id) VALUES ($1, $2, $3)")
                .bind(ws).bind(group).bind(user).execute(&admin).await.unwrap();
        }
        sqlx::query("INSERT INTO fvoci.project_members (id, workspace_id, project_id, group_id, role) VALUES ($1, $2, $3, $4, 'viewer')")
            .bind(Uuid::now_v7()).bind(ws).bind(private_id).bind(group)
            .execute(&admin).await.unwrap();
        sqlx::query("INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role) VALUES ($1, $2, $3, $4, 'viewer')")
            .bind(Uuid::now_v7()).bind(ws).bind(child_id).bind(group)
            .execute(&admin).await.unwrap();
    }
    sqlx::query("INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES ($1, $2, $3, $4, 'viewer')")
        .bind(Uuid::now_v7()).bind(ws).bind(public_id).bind(guest.user_id)
        .execute(&admin).await.unwrap();
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(private_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = $1")
        .bind(child_id)
        .execute(&admin)
        .await
        .unwrap();
    let tag = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.document_tags (id, workspace_id, name, color) VALUES ($1, $2, 'discovery', 'blue')")
        .bind(tag).bind(ws).execute(&admin).await.unwrap();
    for doc in [child_id, private_root.parse().unwrap()] {
        sqlx::query("INSERT INTO fvoci.document_tag_assignments (workspace_id, document_id, tag_id) VALUES ($1, $2, $3)")
            .bind(ws).bind(doc).bind(tag).execute(&admin).await.unwrap();
    }
    let path = format!("/api/v1/workspaces/{ws}/wiki-discovery");
    let ids = |body: &Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (status, owner_rows) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{owner_rows}");
    assert!(ids(&owner_rows).contains(&public_root.to_string()));
    assert!(ids(&owner_rows).contains(&private_root.to_string()));
    assert!(
        !ids(&owner_rows).contains(&hidden_root.to_string()),
        "owner bypassed private ACL"
    );
    let (status, member_rows) =
        json_request(app.clone(), "GET", &path, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK, "{member_rows}");
    assert_eq!(
        ids(&member_rows)
            .iter()
            .filter(|id| id.as_str() == private_root)
            .count(),
        1
    );
    let (status, guest_rows) =
        json_request(app.clone(), "GET", &path, None, Some(&guest.cookie)).await;
    assert_eq!(status, StatusCode::OK, "{guest_rows}");
    let guest_ids = ids(&guest_rows);
    assert!(guest_ids.contains(&public_root.to_string()));
    assert!(guest_ids.contains(&project_child.to_string()));
    assert!(guest_ids.contains(&private_root.to_string()));
    assert_eq!(guest_ids.iter().filter(|id| *id == &wiki[2]).count(), 1);
    assert!(!guest_ids.contains(&wiki[0]));
    assert!(!guest_ids.contains(&wiki[1]));
    assert!(!guest_ids.contains(&hidden_root.to_string()));
    let (status, tagged) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?tag={tag}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tagged}");
    let mut actual = ids(&tagged);
    actual.sort();
    let mut expected = vec![wiki[2].clone(), private_root.to_string()];
    expected.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        tagged["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == wiki[2])
            .unwrap()["parentId"],
        wiki[0]
    );
    for who in [&cookie, &guest.cookie] {
        let (status, legacy) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{ws}/tree"),
            None,
            Some(who),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{legacy}");
        assert!(legacy["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["projectId"].is_null()));
        if who == &guest.cookie {
            assert!(ids(&legacy).is_empty());
        }
    }
    let (status, empty) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?tag={}", Uuid::now_v7()),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids(&empty).is_empty());
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?tag=bad"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for (scope, expected_status) in [
        ("documents.read", StatusCode::OK),
        ("tasks.read", StatusCode::NOT_FOUND),
    ] {
        let (status, created) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/api-tokens"),
            Some(json!({"name": scope, "scopes": [scope]})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let (status, body) = bearer(
            app.clone(),
            "GET",
            &path,
            None,
            created["token"].as_str().unwrap(),
        )
        .await;
        assert_eq!(status, expected_status, "{body}");
    }
    // Hold the row-reading statement after its credential/membership snapshot,
    // then revoke both tags and groups using another committed connection.
    let mut blocker = admin.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("LOCK TABLE fvoci.documents IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pending_app = app.clone();
    let pending_path = format!("{path}?tag={tag}");
    let pending_cookie = guest.cookie.clone();
    let pending = tokio::spawn(async move {
        json_request(
            pending_app,
            "GET",
            &pending_path,
            None,
            Some(&pending_cookie),
        )
        .await
    });
    project_harness::wait_for_blocked_query_count(&admin, pid, "%FROM fvoci.documents d%", 1).await;
    sqlx::query("DELETE FROM fvoci.group_members WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(guest.user_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.document_tag_assignments WHERE workspace_id=$1 AND tag_id=$2")
        .bind(ws)
        .bind(tag)
        .execute(&admin)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let (status, snapshot) = pending.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{snapshot}");
    let mut snapshot_ids = ids(&snapshot);
    snapshot_ids.sort();
    assert_eq!(
        snapshot_ids, expected,
        "permission and tag use the same read snapshot"
    );
    // A new snapshot sees the already-committed revocations.
    let (status, revoked) =
        json_request(app.clone(), "GET", &path, None, Some(&guest.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!ids(&revoked).contains(&private_root.to_string()));
    assert!(!ids(&revoked).contains(&wiki[2]));
    sqlx::query("UPDATE fvoci.projects SET deleted_at = now() WHERE id = $1")
        .bind(public_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(child_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, deleted) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!ids(&deleted).contains(&public_root.to_string()));
    assert!(!ids(&deleted).contains(&wiki[2]));
    let session = session_id_for_user(&admin, guest.user_id).await;
    sqlx::query("DELETE FROM fvoci.sessions WHERE id = $1")
        .bind(session)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(app.clone(), "GET", &path, None, Some(&guest.cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A live credential outside this workspace still cannot read discovery.
    let outsider = add_workspace_user(&admin, ws, "member", "discovery-outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id=$1 AND user_id=$2")
        .bind(ws)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, other) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"slug": "discovery-other", "name": "Other"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let other_ws: Uuid = other["id"].as_str().unwrap().parse().unwrap();
    let foreign_tag = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.document_tags (id,workspace_id,name,color) VALUES ($1,$2,'foreign','blue')")
        .bind(foreign_tag).bind(other_ws).execute(&admin).await.unwrap();
    let (status, foreign) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?tag={foreign_tag}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ids(&foreign).is_empty());
    let (status, _) = json_request(app.clone(), "GET", &path, None, Some(&outsider.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let restricted = app_pool(&harness).await;
    let (superuser, bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&restricted)
            .await
            .unwrap();
    assert!(!superuser && !bypass);
    let unscoped: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.documents")
        .fetch_one(&restricted)
        .await
        .unwrap();
    assert_eq!(unscoped, 0, "tenant context must not survive pooled reads");
    restricted.close().await;
    assert_ne!(owner_id, guest.user_id);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn groups_crud_membership_project_grant_and_user_id_members() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "grp-member").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GRP", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let events_before: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "랩팀"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["name"], "랩팀");
    let group_id = created["id"].as_str().unwrap().to_string();

    let events_after: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        events_after.0, events_before.0,
        "source has no group events"
    );

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == group_id));

    let (status, member_create) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "멤버불가"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{member_create:?}");

    let (status, add) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{add:?}");

    let (status, members) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members["items"][0]["userId"], member.user_id.to_string());

    let (status, grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant:?}");

    let (status, grants) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grants["items"][0]["groupId"], group_id);
    assert_eq!(grants["items"][0]["role"], "viewer");

    let (status, project_get) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{project_get:?}");

    let (status, project_members) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let user_ids: Vec<&str> = project_members["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["userId"].as_str())
        .collect();
    assert!(user_ids.iter().all(|id| Uuid::parse_str(id).is_ok()));
    assert!(!user_ids.contains(&member.user_id.to_string().as_str()));
    assert!(user_ids.contains(&owner_id.to_string().as_str()));

    let (status, ungrant) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ungrant:?}");

    let (status, after) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{after:?}");

    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed:?}");

    let (status, deleted) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn groups_missing_unauth_and_extra_keys() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    let (status, missing) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/groups/00000000-0000-7000-8000-000000000001/members"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing:?}");

    let (status, unauth) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{unauth:?}");

    let (status, bad) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "오염", "evil": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_document_group_grants_and_project_document_404() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GRP", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "위키"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki:?}");
    let wiki_id = wiki["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "위키뷰어"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();

    let guest = add_workspace_user(&admin, workspace_id, "guest", "grp-guest").await;
    let (status, before) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{before:?}");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": guest.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant:?}");

    let (status, grants) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grants["items"][0]["groupId"], group_id);
    assert_eq!(grants["items"][0]["role"], "viewer");

    let (status, opened) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened:?}");

    let (status, guest_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{guest_list:?}");

    let (status, comments) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/comments"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{comments:?}");

    let (status, write_comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/comments"),
        Some(json!({"body": "불가"})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{write_comment:?}");

    let (status, revisions) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/revisions"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revisions:?}");

    let (status, unauth) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{unauth:?}");

    let outsider = add_workspace_user(&admin, workspace_id, "guest", "outsider-tmp").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, outsider_get) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{outsider_get:?}");

    let (status, project_grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{project_grant:?}");

    let (status, project_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{project_list:?}");

    let (status, project_groups) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project_groups["items"].as_array().unwrap().len(), 0);

    let (status, revoked) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked:?}");

    let (status, closed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{closed:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_leave_and_group_delete_cascade_grants() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "cascade").await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "캐스케이드"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let project = create_project(app.clone(), &cookie, workspace_id, "CAS", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id.to_string(), "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            member.user_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let remaining: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.group_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(remaining.0, 0);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let grants: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.project_members WHERE workspace_id = $1 AND group_id = $2",
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(grants.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn last_private_lead_group_delete_conflicts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LEAD", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "책임그룹"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "lead"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{owner_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn groups_rls_force_and_app_role_grants() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "격리"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let flags: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci'
          AND c.relname IN ('groups', 'group_members', 'document_members')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(flags.len(), 3);
    for (name, rls, force) in &flags {
        assert!(rls, "{name} missing RLS");
        assert!(force, "{name} missing FORCE RLS");
    }

    let app_db = app_pool(&harness).await;
    let visible: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.groups")
        .fetch_one(&app_db)
        .await
        .unwrap();
    assert_eq!(visible.0, 0);

    admin.close().await;
    app_db.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn group_pat_scopes_match_source() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PAT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, manage) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "ws", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manage:?}");
    let manage_token = manage["token"].as_str().unwrap();

    let (status, created) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "토큰그룹"})),
        manage_token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();

    let (status, projects_only) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "pr", "scopes": ["projects.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let projects_token = projects_only["token"].as_str().unwrap();
    let (status, denied) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "거부"})),
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    let (status, listed) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");

    let (status, grant_denied) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{grant_denied:?}");

    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "토큰위키"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let wiki_id = wiki["id"].as_str().unwrap();

    let (status, docs) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "docs", "scopes": ["documents.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let docs_token = docs["token"].as_str().unwrap();
    let (status, session_only) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        docs_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{session_only:?}");

    let (status, list_ok) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        docs_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list_ok:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn remove_project_member_returns_not_found_for_group_only_member() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "group-only").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GOM", "private").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "그룹전용"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let events_before: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'project_member.removed'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let audits_before: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'project_member.removed'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();

    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{}",
            member.user_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");

    let events_after: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'project_member.removed'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let audits_after: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'project_member.removed'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events_after.0, events_before.0);
    assert_eq!(audits_after.0, audits_before.0);

    let (status, still) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{still:?}");

    let (status, role_body) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{}",
            member.user_id
        ),
        Some(json!({"role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{role_body:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn update_project_lead_rejects_group_only_member_without_demoting() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "lead-group").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LGO", "private").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "리드후보"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"leadUserId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "conflict");

    let (status, members) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let owner_row = members["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["userId"] == owner_id.to_string())
        .expect("owner remains a direct member");
    assert_eq!(owner_row["role"], "lead");
    assert!(members["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["userId"] != member.user_id.to_string()));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn group_granted_member_sees_private_project_and_labels() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "list-grant").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "VIS", "private").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, label) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels"),
        Some(json!({"name": "버그", "color": "red"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{label:?}");
    let label_id = label["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "목록권한"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == project_id));

    let (status, labels) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/labels"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{labels:?}");
    assert!(labels["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == label_id && item["projectId"] == project_id));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn purge_group_waits_on_member_advisory_lock_then_revokes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "race-purge").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "RCE", "private").await;
    let project_id = project["id"].as_str().unwrap().to_string();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "경합"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap().to_string();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let mut barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    hold_membership_user_lock(&mut barrier, member.user_id).await;

    let purge = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let group_id = group_id.clone();
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
                None,
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, blocker_pid).await;
    assert!(
        !purge.is_finished(),
        "purge must wait for the in-flight member advisory lock"
    );
    barrier.commit().await.unwrap();
    let (status, body) = purge.await.expect("purge join");
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let (status, after) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{after:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_grant_revoke_is_visible_to_collab_acl_poll() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "collab-grant").await;
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "협업문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki:?}");
    let wiki_id: Uuid = wiki["id"].as_str().unwrap().parse().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "위키편집"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap().to_string();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": guest.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let app_db = app_pool(&harness).await;
    let guest_session = session_id_for_user(&admin, guest.user_id).await;
    let admitted = fvoci_server::db::collab_delivery::check_delivery_admission(
        &app_db,
        workspace_id,
        guest.user_id,
        guest_session,
        wiki_id,
    )
    .await
    .unwrap();
    assert_eq!(
        admitted,
        fvoci_server::db::collab_delivery::DeliveryAdmission::Allowed { read_only: false }
    );

    let mut barrier = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    hold_membership_user_lock(&mut barrier, guest.user_id).await;
    let revoke = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let group_id = group_id.clone();
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
                Some(json!({"groupId": group_id})),
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, blocker_pid).await;
    assert!(!revoke.is_finished(), "document grant revoke must wait");
    barrier.commit().await.unwrap();
    let (status, body) = revoke.await.expect("revoke join");
    assert_eq!(status, StatusCode::OK, "{body:?}");

    let denied = fvoci_server::db::collab_delivery::check_delivery_admission(
        &app_db,
        workspace_id,
        guest.user_id,
        guest_session,
        wiki_id,
    )
    .await
    .unwrap();
    assert_eq!(
        denied,
        fvoci_server::db::collab_delivery::DeliveryAdmission::Denied
    );

    app_db.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn group_only_viewer_project_list_reports_can_edit_false() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "viewer-grant").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "VWR", "private").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "뷰어"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, listed) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let row = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == project_id)
        .expect("granted project is listed");
    assert_eq!(row["canEdit"], false);

    admin.close().await;
    harness.cleanup().await;
}

mod sqlite_port {
    use super::*;
    use fvoci_server::auth::password::Keyring;
    use fvoci_server::auth::AuthService;
    use fvoci_server::db::{backend::Backend, migrate, pool, Db};
    use fvoci_server::http::{router, state::AppState};
    use std::sync::Arc;
    fn state(backend: Backend, root: &std::path::Path) -> AppState {
        let auth = Arc::new(AuthService {
            db: Db::from_backend(backend),
            password_keys: Keyring::parse(
                r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                "test",
            )
            .unwrap(),
        });
        AppState {
            realtime_mode: fvoci_server::config::RealtimeMode::Off,
            native_engine: None,
            auth,
            branding_name: "FVOCI".into(),
            public_origin: "http://localhost".into(),
            cookie_secure: false,
            rate_limiter: fvoci_server::http::rate_limit::RateLimiter::new(),
            storage: fvoci_server::attachments::LocalStorage::new(root.join("storage")).into(),
            upload: fvoci_server::attachments::UploadLimits {
                part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
                part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                    fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
                ),
            },
            collab: None,
            meili: None,
            search_embedder: None,
            streams: AppState::fresh_streams(),
            mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
            quota: Default::default(),
            markdown: None,
            import_wake: None,
            import_extractor_available: false,
            preview_extract: None,
        }
    }
    /// First-instance setup through the product route; returns the session cookie.
    async fn setup_owner(app: &axum::Router) -> String {
        let (status,_,headers)=project_harness::http_request(app.clone(),"POST","/api/v1/setup",Some(json!({"email":"owner@example.invalid","password":"Synthetic-Group-2026!","givenName":"Owner","workspaceSlug":"groups","workspaceName":"Groups"}).to_string().into_bytes()),Some("application/json"),None,&[]).await;
        assert_eq!(status, StatusCode::CREATED);
        headers
            .get(axum::http::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .to_string()
    }
    struct Fixture {
        root: std::path::PathBuf,
        path: std::path::PathBuf,
        pool: sqlx::SqlitePool,
        backend: Backend,
        app: axum::Router,
        cookie: String,
        ws: Uuid,
        actor: Uuid,
    }
    impl Fixture {
        async fn start() -> Self {
            let root =
                std::env::temp_dir().join(format!("fvoci-membership-port-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("db.sqlite");
            migrate::run_sqlite_migrations(&path).await.unwrap();
            let sqlite = pool::connect_sqlite_app(&path, 4).await.unwrap();
            let backend = Backend::Sqlite(sqlite.clone());
            let app = router(state(backend.clone(), &root), None);
            let cookie = setup_owner(&app).await;
            let bytes: Vec<u8> =
                sqlx::query_scalar("SELECT id FROM workspaces WHERE slug='groups'")
                    .fetch_one(&sqlite)
                    .await
                    .unwrap();
            let ws = Uuid::from_slice(&bytes).unwrap();
            let bytes: Vec<u8> =
                sqlx::query_scalar("SELECT id FROM users WHERE email='owner@example.invalid'")
                    .fetch_one(&sqlite)
                    .await
                    .unwrap();
            let actor = Uuid::from_slice(&bytes).unwrap();
            Self {
                root,
                path,
                pool: sqlite,
                backend,
                app,
                cookie,
                ws,
                actor,
            }
        }
        async fn member(&self, role: &str) -> (Uuid, String, Uuid) {
            let user = Uuid::now_v7();
            let session = Uuid::now_v7();
            let token = fvoci_server::auth::token::new_token();
            sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Peer')")
                .bind(user.as_bytes().as_slice())
                .bind(format!("{user}@example.invalid"))
                .execute(&self.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
                .bind(self.ws.as_bytes().as_slice())
                .bind(user.as_bytes().as_slice())
                .bind(role)
                .execute(&self.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,(unixepoch()+3600)*1000000)").bind(session.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(token.hash).execute(&self.pool).await.unwrap();
            (user, token.token, session)
        }
        async fn call(
            &self,
            method: &str,
            path: &str,
            body: Option<Value>,
            cookie: &str,
        ) -> (StatusCode, Value) {
            project_harness::json_request(self.app.clone(), method, path, body, Some(cookie)).await
        }
        async fn owner(
            &self,
            method: &str,
            path: &str,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            self.call(method, path, body, &self.cookie).await
        }
        async fn document(&self, parent: Option<Uuid>) -> Uuid {
            let id = Uuid::now_v7();
            let path = if let Some(p) = parent {
                let prefix: String = sqlx::query_scalar("SELECT path FROM documents WHERE id=?1")
                    .bind(p.as_bytes().as_slice())
                    .fetch_one(&self.pool)
                    .await
                    .unwrap();
                format!("{prefix}.{}", id.simple())
            } else {
                id.simple().to_string()
            };
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,parent_id,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Visible',?3,?4,'a',1,'published',1,?5,?6)").bind(id.as_bytes().as_slice()).bind(self.ws.as_bytes().as_slice()).bind(path).bind(parent.map(|id|id.as_bytes().to_vec())).bind(self.actor.as_bytes().as_slice()).bind(json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Shared text"}]}]}).to_string()).execute(&self.pool).await.unwrap();
            id
        }
        async fn foreign(&self) -> Uuid {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,'Foreign')")
                .bind(id.as_bytes().as_slice())
                .bind(id.simple().to_string())
                .execute(&self.pool)
                .await
                .unwrap();
            id
        }
        async fn count(&self, table: &str) -> i64 {
            let fresh = pool::connect_sqlite_app(&self.path, 1).await.unwrap();
            let n = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&fresh)
                .await
                .unwrap();
            fresh.close().await;
            n
        }
        async fn close(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    #[tokio::test]
    async fn sqlite_groups_members_and_grants_enforce_current_authority() {
        let f = Fixture::start().await;
        let (guest, cookie, _) = f.member("guest").await;
        let foreign = f.foreign().await;
        let base = format!("/api/v1/workspaces/{}/groups", f.ws);
        let (s, g) = f.owner("POST", &base, Some(json!({"name":"Team"}))).await;
        assert_eq!(s, StatusCode::CREATED, "{g}");
        let group = g["id"].as_str().unwrap();
        let members = format!("{base}/{group}/members");
        let body = json!({"userId":guest});
        assert_eq!(
            f.owner("POST", &members, Some(body.clone())).await.0,
            StatusCode::CREATED
        );
        assert_eq!(
            f.owner("POST", &members, Some(body.clone())).await.0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.owner("GET", &members, None).await.1["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'GROUP','Private','private',?3)").bind(project.as_bytes().as_slice()).bind(f.ws.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.ws.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let wiki = f.document(None).await;
        for (domain, target) in [("projects", project), ("documents", wiki)] {
            let path = format!("/api/v1/workspaces/{}/{domain}/{target}/groups", f.ws);
            assert_eq!(
                f.call("GET", &path, None, &cookie).await.0,
                StatusCode::NOT_FOUND
            );
            let grant = json!({"groupId":group,"role":"member"});
            assert_eq!(
                f.owner("POST", &path, Some(grant.clone())).await.0,
                StatusCode::CREATED
            );
            assert_eq!(
                f.owner("POST", &path, Some(grant.clone())).await.0,
                StatusCode::CONFLICT
            );
            assert_eq!(
                f.owner("GET", &path, None).await.1["items"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(f.call("GET", &path, None, &cookie).await.0, StatusCode::OK);
            assert_eq!(
                f.call("DELETE", &path, Some(json!({"groupId":group})), &cookie)
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
            let other = format!("/api/v1/workspaces/{foreign}/{domain}/{target}/groups");
            for method in ["GET", "POST", "DELETE"] {
                assert_eq!(
                    f.owner(
                        method,
                        &other,
                        if method == "GET" {
                            None
                        } else {
                            Some(if method == "POST" {
                                grant.clone()
                            } else {
                                json!({"groupId":group})
                            })
                        }
                    )
                    .await
                    .0,
                    StatusCode::NOT_FOUND
                );
            }
            assert_eq!(
                f.owner("DELETE", &path, Some(json!({"groupId":group})))
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(
                f.call("GET", &path, None, &cookie).await.0,
                StatusCode::NOT_FOUND
            );
        }
        // A group can be the only lead; neither grant removal nor purge may orphan it.
        assert_eq!(
            f.owner("POST", &members, Some(json!({"userId":f.actor})))
                .await
                .0,
            StatusCode::CREATED
        );
        sqlx::query("UPDATE project_members SET user_id=NULL,group_id=?1 WHERE project_id=?2")
            .bind(Uuid::parse_str(group).unwrap().as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let path = format!("/api/v1/workspaces/{}/projects/{project}/groups", f.ws);
        assert_eq!(
            f.owner("DELETE", &path, Some(json!({"groupId":group})))
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.owner("DELETE", &format!("{base}/{group}"), None).await.0,
            StatusCode::CONFLICT
        );
        assert_eq!(f.count("groups").await, 1);
        sqlx::query("UPDATE projects SET visibility='workspace' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            f.owner("DELETE", &members, Some(body.clone())).await.0,
            StatusCode::OK
        );
        assert_eq!(
            f.owner("DELETE", &members, Some(body)).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.owner("DELETE", &format!("{base}/{group}"), None).await.0,
            StatusCode::OK
        );
        assert_eq!(f.count("groups").await, 0);
        f.close().await;
    }

    /// Writes that have no NUL input validator; PostgreSQL refuses each one at
    /// its text bind (SQLSTATE 22021).
    fn nul_text_requests(ws: Uuid) -> [(&'static str, String, Value); 3] {
        [
            (
                "POST",
                format!("/api/v1/workspaces/{ws}/groups"),
                json!({"name": "Team\u{0}A"}),
            ),
            (
                "PATCH",
                format!("/api/v1/workspaces/{ws}"),
                json!({"name": "Renamed\u{0}A"}),
            ),
            (
                "POST",
                format!("/api/v1/workspaces/{ws}/api-tokens"),
                json!({"name": "token\u{0}A", "scopes": ["tasks.read"]}),
            ),
        ]
    }

    /// What PostgreSQL 16-18 answers today for each refusal these tests pin.
    fn postgres_internal_error() -> (StatusCode, Value) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"type":"about:blank","title":"internal error","status":500,"code":"internal_error"}),
        )
    }

    #[tokio::test]
    async fn nul_text_without_validator_matches_postgres_and_stores_nothing() {
        type Stored = (i64, i64, i64, i64, String);
        async fn pg_stored(admin: &sqlx::PgPool, ws: Uuid) -> Stored {
            sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.groups),(SELECT count(*) FROM fvoci.api_tokens),(SELECT count(*) FROM fvoci.events),(SELECT count(*) FROM fvoci.audit_log),(SELECT name FROM fvoci.workspaces WHERE id=$1)")
                .bind(ws)
                .fetch_one(admin)
                .await
                .unwrap()
        }
        async fn sqlite_stored(pool: &sqlx::SqlitePool, ws: Uuid) -> Stored {
            sqlx::query_as("SELECT (SELECT count(*) FROM groups),(SELECT count(*) FROM api_tokens),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log),(SELECT name FROM workspaces WHERE id=?1)")
                .bind(ws.as_bytes().as_slice())
                .fetch_one(pool)
                .await
                .unwrap()
        }

        let harness = TestDb::bootstrap().await;
        let (app, cookie, _, ws) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let refused = sqlx::query_scalar::<_, String>("SELECT $1::text")
            .bind("a\u{0}b")
            .fetch_one(&admin)
            .await
            .unwrap_err();
        assert_eq!(
            refused
                .as_database_error()
                .and_then(|e| e.code())
                .as_deref(),
            Some("22021")
        );
        let before = pg_stored(&admin, ws).await;
        let mut postgres = Vec::new();
        for (method, path, body) in nul_text_requests(ws) {
            postgres
                .push(json_request(app.clone(), method, &path, Some(body), Some(&cookie)).await);
        }
        assert_eq!(pg_stored(&admin, ws).await, before);
        assert!(
            postgres.iter().all(|r| *r == postgres_internal_error()),
            "{postgres:?}"
        );
        admin.close().await;
        harness.cleanup().await;

        let f = Fixture::start().await;
        let before = sqlite_stored(&f.pool, f.ws).await;
        let mut sqlite = Vec::new();
        for (method, path, body) in nul_text_requests(f.ws) {
            sqlite.push(f.owner(method, &path, Some(body)).await);
        }
        assert_eq!(sqlite, postgres);
        assert_eq!(sqlite_stored(&f.pool, f.ws).await, before);
        f.close().await;
    }

    /// A schema-current app over the remote libSQL driver against the pinned
    /// official sqld, with the first owner signed in.
    struct Remote {
        root: std::path::PathBuf,
        driver: libsql_fixture::LibsqlFinishFixture,
        connection: libsql::Connection,
        backend: Backend,
        app: axum::Router,
        cookie: String,
        ws: Uuid,
    }
    impl Remote {
        async fn start(label: &str) -> Self {
            use fvoci_server::db::backend::RemoteDatabase;
            let sqld = std::path::PathBuf::from(
                std::env::var_os("FVOCI_TEST_SQLD")
                    .expect("explicit pinned official sqld required"),
            );
            let root = std::env::temp_dir().join(format!("fvoci-{label}-sqld-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let driver = libsql_fixture::LibsqlFinishFixture::start(&sqld, &root.join("sqld"))
                .await
                .unwrap();
            let connection = driver.database().await.unwrap().connect().unwrap();
            for step in migrate::compiled_sqlite_steps() {
                let tx = connection
                    .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                    .await
                    .unwrap();
                tx.execute_batch(step.sql).await.unwrap();
                tx.execute("INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(?1,?2,?3,unixepoch()*1000000)",
                    libsql::params![i64::from(step.version), migrate::SQLITE_LINEAGE, step.sha256]).await.unwrap();
                tx.commit().await.unwrap();
            }
            let backend = Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(
                driver.database().await.unwrap(),
                std::num::NonZeroU32::new(4).unwrap(),
            )));
            migrate::assert_sqlite_schema_current(&backend)
                .await
                .unwrap();
            let app = router(state(backend.clone(), &root), None);
            let cookie = setup_owner(&app).await;
            let mut rows = connection
                .query("SELECT id FROM workspaces WHERE slug='groups'", ())
                .await
                .unwrap();
            let id: Vec<u8> = rows.next().await.unwrap().unwrap().get(0).unwrap();
            Self {
                root,
                driver,
                connection,
                backend,
                app,
                cookie,
                ws: Uuid::from_slice(&id).unwrap(),
            }
        }
        async fn row(&self, sql: &str) -> libsql::Row {
            let mut rows = self.connection.query(sql, ()).await.unwrap();
            rows.next().await.unwrap().unwrap()
        }
        async fn finish(self) {
            let Self {
                root,
                driver,
                connection,
                backend,
                app,
                ..
            } = self;
            drop(app);
            drop(connection);
            backend.close().await.unwrap();
            let receipt = driver.finish().await.unwrap();
            assert!(receipt.sqld_reaped && receipt.proxy_joined);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// The same requests through the remote libSQL driver against the pinned
    /// official sqld: the PostgreSQL response, and nothing stored remotely.
    #[tokio::test]
    #[ignore = "explicit pinned official sqld fixture via FVOCI_TEST_SQLD"]
    async fn remote_nul_text_without_validator_matches_postgres_and_stores_nothing() {
        async fn stored(r: &Remote) -> (Vec<i64>, String) {
            let row = r
                .row("SELECT (SELECT count(*) FROM groups),(SELECT count(*) FROM api_tokens),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log),(SELECT name FROM workspaces WHERE slug='groups')")
                .await;
            let counts = (0..4).map(|i| row.get::<i64>(i).unwrap()).collect();
            (counts, row.get(4).unwrap())
        }
        let r = Remote::start("nul-text").await;
        let before = stored(&r).await;
        for (method, path, body) in nul_text_requests(r.ws) {
            let response = project_harness::json_request(
                r.app.clone(),
                method,
                &path,
                Some(body),
                Some(&r.cookie),
            )
            .await;
            assert_eq!(response, postgres_internal_error(), "{method} {path}");
        }
        assert_eq!(stored(&r).await, before);
        r.finish().await;
    }

    /// A timer record correction keeps its note only in the audit and receipt
    /// JSON, and no validator refuses NUL there. PostgreSQL stores both as
    /// jsonb, so the note is refused with SQLSTATE 22P05. Creates the manual
    /// record and returns the correction request (path, body).
    async fn timer_note_correction(app: &axum::Router, cookie: &str, ws: Uuid) -> (String, Value) {
        let project = create_project(app.clone(), cookie, ws, "JSONNUL", "workspace").await;
        let (status, task) = json_request(
            app.clone(),
            "POST",
            &format!(
                "/api/v1/workspaces/{ws}/projects/{}/tasks",
                project["id"].as_str().unwrap()
            ),
            Some(json!({"title": "Timer"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        let base = format!(
            "/api/v1/workspaces/{ws}/tasks/{}",
            task["id"].as_str().unwrap()
        );
        let (status, me) =
            json_request(app.clone(), "GET", "/api/v1/auth/me", None, Some(cookie)).await;
        assert_eq!(status, StatusCode::OK, "{me}");
        let context = |mut body: Value| {
            body["expectedActorId"] = me["userId"].clone();
            body["expectedSessionId"] = me["sessionId"].clone();
            body
        };
        let (status, created) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/timer/history"),
            Some(context(json!({"requestId": Uuid::now_v7(), "startedAt": "2026-09-30T10:00:00Z", "endedAt": "2026-09-30T10:00:02Z", "note": "manual", "reason": "Manual entry"}))),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let record = &created["record"];
        (
            format!(
                "{base}/timer/records/{}/correct",
                record["id"].as_str().unwrap()
            ),
            context(
                json!({"requestId": Uuid::now_v7(), "kind": "manual", "expectedRevision": record["revision"], "expectedStartedAt": record["startedAt"], "expectedEndedAt": record["endedAt"], "expectedNote": record["note"], "startedAt": "2026-09-30T10:00:00Z", "endedAt": "2026-09-30T10:00:01Z", "note": "fixed\u{0}note", "reason": "Shorter range"}),
            ),
        )
    }

    #[tokio::test]
    async fn json_nul_without_validator_matches_postgres_and_stores_nothing() {
        type Stored = (i64, i64, i64, i64, i64);
        async fn pg_stored(admin: &sqlx::PgPool) -> Stored {
            sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.task_timer_audit),(SELECT count(*) FROM fvoci.task_timer_commands),(SELECT count(*) FROM fvoci.time_entries),(SELECT count(*) FROM fvoci.events),(SELECT count(*) FROM fvoci.audit_log)")
                .fetch_one(admin)
                .await
                .unwrap()
        }
        async fn sqlite_stored(pool: &sqlx::SqlitePool) -> Stored {
            sqlx::query_as("SELECT (SELECT count(*) FROM task_timer_audit),(SELECT count(*) FROM task_timer_commands),(SELECT count(*) FROM time_entries),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)")
                .fetch_one(pool)
                .await
                .unwrap()
        }

        let harness = TestDb::bootstrap().await;
        let (app, cookie, _, ws) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        // jsonb refuses U+0000 in a string or a key, however the text escapes
        // it, and keeps the six characters of a literal `\u0000`.
        for (value, code) in [
            (json!({"note": "a\u{0}b"}), Some("22P05")),
            (json!({"k\u{0}": 1}), Some("22P05")),
            (json!({"note": "\\u0000"}), None),
        ] {
            let stored = sqlx::query_scalar::<_, Value>("SELECT $1::jsonb")
                .bind(value.clone())
                .fetch_one(&admin)
                .await;
            assert_eq!(
                stored
                    .as_ref()
                    .err()
                    .and_then(|e| e.as_database_error())
                    .and_then(|e| e.code())
                    .as_deref(),
                code,
                "{value}"
            );
        }
        let (path, body) = timer_note_correction(&app, &cookie, ws).await;
        let before = pg_stored(&admin).await;
        let postgres = json_request(app.clone(), "POST", &path, Some(body), Some(&cookie)).await;
        assert_eq!(postgres, postgres_internal_error());
        assert_eq!(pg_stored(&admin).await, before);
        admin.close().await;
        harness.cleanup().await;

        let f = Fixture::start().await;
        let (path, body) = timer_note_correction(&f.app, &f.cookie, f.ws).await;
        let before = sqlite_stored(&f.pool).await;
        assert_eq!(f.owner("POST", &path, Some(body)).await, postgres);
        assert_eq!(sqlite_stored(&f.pool).await, before);
        f.close().await;
    }

    /// 321 characters: the request route sets no length limit, and
    /// PostgreSQL's app_user_update_email refuses it when the link is used.
    fn overlong_email() -> String {
        format!("{}@example.com", "a".repeat(309))
    }
    /// 320 characters, the longest address app_user_update_email accepts.
    fn longest_email() -> String {
        format!("{}@example.com", "b".repeat(308))
    }
    /// Requests an email change and returns a token for its link. The mailer
    /// is off in these apps, so the caller stores this token's hash on it.
    async fn request_email_change(
        app: &axum::Router,
        cookie: &str,
        email: &str,
    ) -> fvoci_server::auth::token::SessionToken {
        let (status, body) = json_request(
            app.clone(),
            "PATCH",
            "/api/v1/auth/email",
            Some(json!({"newEmail": email})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        fvoci_server::auth::token::new_token()
    }
    async fn confirm_email_change(app: &axum::Router, token: &str) -> (StatusCode, Value) {
        json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/email/confirm",
            Some(json!({"token": token})),
            None,
        )
        .await
    }
    /// The SQLSTATE each backend's email change raises for a multibyte address
    /// (172 characters, 332 bytes) and for the 321-character one.
    async fn email_refusal_codes(
        backend: &Backend,
        user: Uuid,
        generation: i32,
    ) -> Vec<Option<String>> {
        use fvoci_server::db::magic::{MagicPayload, MAGIC_KIND_EMAIL_CHANGE};
        let mut codes = Vec::new();
        for email in [format!("{}@example.com", "é".repeat(160)), overlong_email()] {
            let payload = MagicPayload {
                kind: MAGIC_KIND_EMAIL_CHANGE.into(),
                user_id: user,
                generation,
                new_email: Some(email),
            };
            let error =
                fvoci_server::db::account::complete_email_change_backend(backend, &payload, None)
                    .await
                    .err()
                    .expect("email change refused");
            codes.push(
                error
                    .as_database_error()
                    .and_then(|e| e.code())
                    .map(|code| code.into_owned()),
            );
        }
        codes
    }

    /// (response, address changed, auth generation step, email_changed events
    /// added, links left) for the 321- then the 320-character address.
    type EmailOutcome = (StatusCode, Value, bool, i64, i64, i64);

    #[tokio::test]
    async fn overlong_email_change_matches_postgres_and_changes_nothing() {
        type Stored = (String, i64, i64, i64);
        async fn pg_stored(admin: &sqlx::PgPool, user: Uuid) -> Stored {
            sqlx::query_as("SELECT email,auth_generation::bigint,(SELECT count(*) FROM fvoci.events WHERE verb='user.email_changed'),(SELECT count(*) FROM fvoci.magic_tokens WHERE kind='email_change') FROM fvoci.users WHERE id=$1")
                .bind(user)
                .fetch_one(admin)
                .await
                .unwrap()
        }
        async fn sqlite_stored(pool: &sqlx::SqlitePool, user: Uuid) -> Stored {
            sqlx::query_as("SELECT email,auth_generation,(SELECT count(*) FROM events WHERE verb='user.email_changed'),(SELECT count(*) FROM magic_tokens WHERE kind='email_change') FROM users WHERE id=?1")
                .bind(user.as_bytes().as_slice())
                .fetch_one(pool)
                .await
                .unwrap()
        }
        fn outcome(response: (StatusCode, Value), before: &Stored, after: &Stored) -> EmailOutcome {
            (
                response.0,
                response.1,
                after.0 != before.0,
                after.1 - before.1,
                after.2 - before.2,
                after.3,
            )
        }

        let harness = TestDb::bootstrap().await;
        let (app, cookie, user, _) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let mut postgres = Vec::new();
        for email in [overlong_email(), longest_email()] {
            let before = pg_stored(&admin, user).await;
            let token = request_email_change(&app, &cookie, &email).await;
            sqlx::query("UPDATE fvoci.magic_tokens SET token_hash=$1 WHERE kind='email_change'")
                .bind(&token.hash)
                .execute(&admin)
                .await
                .unwrap();
            let response = confirm_email_change(&app, &token.token).await;
            postgres.push(outcome(response, &before, &pg_stored(&admin, user).await));
        }
        assert_eq!(
            postgres,
            [
                {
                    let (status, body) = postgres_internal_error();
                    (status, body, false, 0, 0, 0)
                },
                (StatusCode::OK, json!({"ok": true}), true, 1, 1, 0),
            ]
        );
        let changed = pg_stored(&admin, user).await;
        assert_eq!(changed.0, longest_email());
        let pool = app_pool(&harness).await;
        let postgres_codes = email_refusal_codes(
            &Backend::Postgres(pool.clone()),
            user,
            i32::try_from(changed.1).unwrap(),
        )
        .await;
        project_harness::close_pool(pool).await;
        assert_eq!(
            postgres_codes,
            [Some("P0001".to_string()), Some("P0001".to_string())]
        );
        assert_eq!(pg_stored(&admin, user).await, changed);
        admin.close().await;
        harness.cleanup().await;

        let f = Fixture::start().await;
        let mut sqlite = Vec::new();
        for email in [overlong_email(), longest_email()] {
            let before = sqlite_stored(&f.pool, f.actor).await;
            let token = request_email_change(&f.app, &f.cookie, &email).await;
            sqlx::query("UPDATE magic_tokens SET token_hash=?1 WHERE kind='email_change'")
                .bind(&token.hash)
                .execute(&f.pool)
                .await
                .unwrap();
            let response = confirm_email_change(&f.app, &token.token).await;
            sqlite.push(outcome(
                response,
                &before,
                &sqlite_stored(&f.pool, f.actor).await,
            ));
        }
        assert_eq!(sqlite, postgres);
        let changed = sqlite_stored(&f.pool, f.actor).await;
        assert_eq!(changed.0, longest_email());
        assert_eq!(
            email_refusal_codes(&f.backend, f.actor, i32::try_from(changed.1).unwrap()).await,
            postgres_codes
        );
        assert_eq!(sqlite_stored(&f.pool, f.actor).await, changed);
        f.close().await;
    }

    /// A private project led only by the workspace owner's user grant.
    async fn private_project(app: &axum::Router, cookie: &str, ws: Uuid) -> String {
        let project = create_project(app.clone(), cookie, ws, "PRIV", "private").await;
        project["id"].as_str().unwrap().to_string()
    }
    async fn group_lead(app: &axum::Router, cookie: &str, ws: Uuid, project: &str) {
        let (status, group) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/groups"),
            Some(json!({"name": "Leads"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{group}");
        let (status, grant) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/projects/{project}/groups"),
            Some(json!({"groupId": group["id"], "role": "lead"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{grant}");
    }
    async fn trash(app: &axum::Router, cookie: &str, ws: Uuid, slug: &str) -> (StatusCode, Value) {
        json_request(
            app.clone(),
            "DELETE",
            &format!("/api/v1/workspaces/{ws}"),
            Some(json!({"confirmSlug": slug})),
            Some(cookie),
        )
        .await
    }

    /// Trash cascades every user grant away. PostgreSQL's deferred
    /// project_members_private_lead_check_del then refuses the commit while a
    /// live private project keeps no lead, and passes once a group leads it.
    #[tokio::test]
    async fn trash_leaving_private_project_without_lead_matches_postgres() {
        /// (trashed, memberships, project grants, workspace.deleted events)
        type Stored = (bool, i64, i64, i64);
        async fn pg_stored(admin: &sqlx::PgPool, ws: Uuid) -> Stored {
            sqlx::query_as("SELECT (SELECT deleted_at IS NOT NULL FROM fvoci.workspaces WHERE id=$1),(SELECT count(*) FROM fvoci.memberships WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.project_members WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.events WHERE verb='workspace.deleted')")
                .bind(ws)
                .fetch_one(admin)
                .await
                .unwrap()
        }
        async fn sqlite_stored(pool: &sqlx::SqlitePool, ws: Uuid) -> Stored {
            sqlx::query_as("SELECT (SELECT deleted_at IS NOT NULL FROM workspaces WHERE id=?1),(SELECT count(*) FROM memberships WHERE workspace_id=?1),(SELECT count(*) FROM project_members WHERE workspace_id=?1),(SELECT count(*) FROM events WHERE verb='workspace.deleted')")
                .bind(ws.as_bytes().as_slice())
                .fetch_one(pool)
                .await
                .unwrap()
        }

        let harness = TestDb::bootstrap().await;
        let (app, cookie, _, ws) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = private_project(&app, &cookie, ws).await;
        let mut postgres = vec![pg_stored(&admin, ws).await];
        let refused = trash(&app, &cookie, ws, "acme").await;
        postgres.push(pg_stored(&admin, ws).await);
        group_lead(&app, &cookie, ws, &project).await;
        postgres.push(pg_stored(&admin, ws).await);
        let trashed = trash(&app, &cookie, ws, "acme").await;
        postgres.push(pg_stored(&admin, ws).await);
        assert_eq!(refused, postgres_internal_error(), "{postgres:?}");
        assert_eq!(trashed, (StatusCode::OK, json!({"ok": true})));
        assert_eq!(
            postgres,
            [
                (false, 1, 1, 0),
                (false, 1, 1, 0),
                (false, 1, 2, 0),
                (true, 0, 1, 1)
            ]
        );
        admin.close().await;
        harness.cleanup().await;

        let f = Fixture::start().await;
        let project = private_project(&f.app, &f.cookie, f.ws).await;
        let mut sqlite = vec![sqlite_stored(&f.pool, f.ws).await];
        assert_eq!(trash(&f.app, &f.cookie, f.ws, "groups").await, refused);
        sqlite.push(sqlite_stored(&f.pool, f.ws).await);
        group_lead(&f.app, &f.cookie, f.ws, &project).await;
        sqlite.push(sqlite_stored(&f.pool, f.ws).await);
        assert_eq!(trash(&f.app, &f.cookie, f.ws, "groups").await, trashed);
        sqlite.push(sqlite_stored(&f.pool, f.ws).await);
        assert_eq!(sqlite, postgres);
        f.close().await;
    }

    /// The JSON NUL, overlong email and leadless trash refusals through the
    /// remote libSQL driver: the PostgreSQL responses, and nothing stored.
    #[tokio::test]
    #[ignore = "explicit pinned official sqld fixture via FVOCI_TEST_SQLD"]
    async fn remote_json_nul_email_and_trash_refusals_match_postgres() {
        let r = Remote::start("parity").await;
        let ints = |row: libsql::Row, n: i32| -> Vec<i64> {
            (0..n).map(|i| row.get::<i64>(i).unwrap()).collect()
        };

        let (path, body) = timer_note_correction(&r.app, &r.cookie, r.ws).await;
        let timer = "SELECT (SELECT count(*) FROM task_timer_audit),(SELECT count(*) FROM task_timer_commands),(SELECT count(*) FROM time_entries)";
        let before = ints(r.row(timer).await, 3);
        assert_eq!(
            json_request(r.app.clone(), "POST", &path, Some(body), Some(&r.cookie)).await,
            postgres_internal_error()
        );
        assert_eq!(ints(r.row(timer).await, 3), before);

        let account = "SELECT (SELECT count(*) FROM users WHERE email='owner@example.invalid'),(SELECT max(auth_generation) FROM users),(SELECT count(*) FROM events WHERE verb='user.email_changed')";
        let before = ints(r.row(account).await, 3);
        let token = request_email_change(&r.app, &r.cookie, &overlong_email()).await;
        r.connection
            .execute(
                "UPDATE magic_tokens SET token_hash=?1 WHERE kind='email_change'",
                libsql::params![token.hash.clone()],
            )
            .await
            .unwrap();
        assert_eq!(
            confirm_email_change(&r.app, &token.token).await,
            postgres_internal_error()
        );
        assert_eq!(ints(r.row(account).await, 3), before);

        private_project(&r.app, &r.cookie, r.ws).await;
        let workspace = "SELECT (SELECT deleted_at IS NOT NULL FROM workspaces WHERE slug='groups'),(SELECT count(*) FROM memberships),(SELECT count(*) FROM project_members),(SELECT count(*) FROM events WHERE verb='workspace.deleted')";
        let before = ints(r.row(workspace).await, 4);
        assert_eq!(
            trash(&r.app, &r.cookie, r.ws, "groups").await,
            postgres_internal_error()
        );
        assert_eq!(ints(r.row(workspace).await, 4), before);
        r.finish().await;
    }

    // This binary runs in PostgreSQL shard b, which supplies FVOCI_COLLAB_ENGINE.
    #[tokio::test]
    async fn sqlite_active_room_closes_demoted_and_removed_members() {
        use fvoci_server::collab::{
            config::{CollabConfig, FamilyRoomTimings},
            hub::CollabHub,
            room::{AuthenticatedConnection, CollabSession, RoomJoin, RoomKey},
        };
        let f = Fixture::start().await;
        let (demoted, _, demoted_session) = f.member("admin").await;
        let (removed, _, removed_session) = f.member("member").await;
        let (status, doc) = f
            .owner(
                "POST",
                &format!("/api/v1/workspaces/{}/documents", f.ws),
                Some(json!({"commandId":Uuid::now_v7(),"parentId":null,"title":"Room"})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{doc}");
        let document = Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap();
        let key = RoomKey::document(f.ws, document);
        let mut config = CollabConfig::from_env().expect("allocated built collab engine required");
        config.revoke_poll_ms = 50;
        config.revision_session_snapshot = false;
        let hub = CollabHub::new_backend(
            config,
            f.backend.clone(),
            Some(FamilyRoomTimings::new(30_000, 5_000).unwrap()),
        )
        .unwrap();
        let mut peers = Vec::new();
        for (index, (user, session)) in [(demoted, demoted_session), (removed, removed_session)]
            .into_iter()
            .enumerate()
        {
            let (events, receive) = tokio::sync::mpsc::channel(64);
            let (cancel, cancelled) = tokio::sync::watch::channel(None);
            let lease = hub
                .join_room(
                    key,
                    RoomJoin {
                        conn: AuthenticatedConnection {
                            conn_id: Uuid::now_v7(),
                            session: CollabSession {
                                session_id: session,
                                user_id: user,
                                given_name: "Peer".into(),
                                family_name: None,
                                locale: "en".into(),
                            },
                            client_id: 701 + index as u32,
                            read_only: false,
                            routing_key: format!("{}:{document}", f.ws),
                        },
                        events,
                        cancel: Some(cancel),
                    },
                )
                .await
                .unwrap();
            peers.push((lease, receive, cancelled));
        }
        for (index, method, user, body) in [
            (0, "PATCH", demoted, Some(json!({"role":"guest"}))),
            (1, "DELETE", removed, None),
        ] {
            let (status, body) = f
                .owner(
                    method,
                    &format!("/api/v1/workspaces/{}/members/{user}", f.ws),
                    body,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let cancel = &mut peers[index].2;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if cancel.borrow().is_some() {
                        break;
                    }
                    cancel.changed().await.unwrap();
                }
            })
            .await
            .expect("current membership must close the active room");
            assert_eq!(cancel.borrow().as_ref().unwrap().code, 1008);
        }
        drop(peers);
        assert!(hub.shutdown().await.is_clean());
        f.close().await;
    }
}
