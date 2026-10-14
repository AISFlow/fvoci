#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, drop_insert_fail_trigger,
    http_request, install_insert_fail_trigger, json_request, setup_session, test_peer,
    wait_for_query_blocked_by, TestDb,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

mod sqlite_port {
    use super::*;
    use fvoci_server::auth::{password::Keyring, AuthService};
    use fvoci_server::db::{backend::Backend, migrate, pool, Db};
    use fvoci_server::http::{rate_limit::RateLimiter, state::AppState};
    use std::{path::PathBuf, sync::Arc};

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
        pool: sqlx::SqlitePool,
        app: axum::Router,
        workspace: Uuid,
        actor: Uuid,
        session: Uuid,
        cookie: String,
        project: Uuid,
        document: Uuid,
        project_document: Uuid,
        task: Uuid,
    }

    fn app(backend: Backend, root: &std::path::Path) -> axum::Router {
        fvoci_server::http::router(AppState {
            realtime_mode: fvoci_server::config::RealtimeMode::On,
            native_engine: None,
            auth: Arc::new(AuthService {
                db: Db::from_backend(backend),
                password_keys: Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#, "test").unwrap(),
            }),
            branding_name: "Comments".into(),
            public_origin: "http://localhost".into(),
            cookie_secure: false,
            rate_limiter: RateLimiter::new(),
            storage: fvoci_server::attachments::LocalStorage::new(root.join("storage")).into(),
            upload: fvoci_server::attachments::UploadLimits {
                part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
                max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
                create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
                part_put_slots: fvoci_server::attachments::PartPutSlots::new(fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS),
            },
            collab: None, meili: None, search_embedder: None, markdown: None,
            import_wake: None, import_extractor_available: false, preview_extract: None,
            quota: Default::default(), mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
            streams: AppState::fresh_streams(),
        }, None)
    }

    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("comment-port-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("comments.sqlite");
            migrate::run_sqlite_migrations(&path).await.unwrap();
            let pool = pool::connect_sqlite_app(&path, 3).await.unwrap();
            let workspace = Uuid::now_v7();
            let actor = Uuid::now_v7();
            let session = Uuid::now_v7();
            let token = fvoci_server::auth::token::new_token();
            sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'comments@example.invalid','Author')").bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'comments','Comments')")
                .bind(workspace.as_bytes().as_slice())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
                .bind(workspace.as_bytes().as_slice())
                .bind(actor.as_bytes().as_slice())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,(unixepoch()+3600)*1000000)").bind(session.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).bind(&token.hash).execute(&pool).await.unwrap();
            let project = Uuid::now_v7();
            let workflow = Uuid::now_v7();
            let status = Uuid::now_v7();
            sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'COM','Comments','workspace',?3)").bind(project.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
                .bind(workflow.as_bytes().as_slice())
                .bind(workspace.as_bytes().as_slice())
                .bind(project.as_bytes().as_slice())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'Todo','todo','V')").bind(status.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&pool).await.unwrap();
            let task = Uuid::now_v7();
            sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by) VALUES(?1,?2,?3,1,'Task',?4,'{}',?5)").bind(task.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            let document = Uuid::now_v7();
            let project_document = Uuid::now_v7();
            for (id, project_id, number) in
                [(document, None, 1), (project_document, Some(project), 2)]
            {
                sqlx::query("INSERT INTO documents(id,workspace_id,project_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,?3,'Document',?4,'V',?5,'published',2,?6,'{}')").bind(id.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project_id.map(|id|id.as_bytes().to_vec())).bind(id.simple().to_string()).bind(number).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            }
            let app = app(Backend::Sqlite(pool.clone()), &root);
            Self {
                root,
                path,
                pool,
                app,
                workspace,
                actor,
                session,
                cookie: token.token,
                project,
                document,
                project_document,
                task,
            }
        }

        fn paths(&self) -> [String; 3] {
            let w = self.workspace;
            [
                format!(
                    "/api/v1/workspaces/{w}/documents/{}/comments",
                    self.document
                ),
                format!(
                    "/api/v1/workspaces/{w}/projects/{}/documents/{}/comments",
                    self.project, self.project_document
                ),
                format!("/api/v1/workspaces/{w}/tasks/{}/comments", self.task),
            ]
        }

        fn comment_path(&self, comment: &Value) -> String {
            format!(
                "/api/v1/workspaces/{}/comments/{}",
                self.workspace,
                comment["id"].as_str().unwrap()
            )
        }

        async fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            json_request(self.app.clone(), method, path, body, Some(&self.cookie)).await
        }

        async fn close(self) {
            drop(self.app);
            self.pool.close().await;
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    #[tokio::test]
    async fn sqlite_comments_http_lifecycle_and_fresh_readback() {
        let f = Fixture::new().await;
        let (mentioned, _, _) = user(&f, Some("member")).await;
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Mentions')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for actor in [f.actor, mentioned] {
            sqlx::query(
                "INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        }
        for (index, path) in f.paths().iter().enumerate() {
            let (status, created) = f.request("POST",path,Some(json!({"body":" 댓글 😀 ","mentionedUserIds":[f.actor,f.actor],"mentionedGroupIds":[group,group]}))).await;
            assert_eq!(status, StatusCode::CREATED, "{created}");
            assert_eq!(created["body"], "댓글 😀");
            assert_eq!(created["createdBy"], f.actor.to_string());
            assert_eq!(created["workspaceId"], f.workspace.to_string());
            if index == 2 {
                assert_eq!(created["taskId"], f.task.to_string());
                assert!(created["documentId"].is_null());
            } else {
                assert_eq!(
                    created["documentId"],
                    if index == 0 {
                        f.document
                    } else {
                        f.project_document
                    }
                    .to_string()
                );
                assert!(created["taskId"].is_null());
            }
            let comment_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
            let fresh = pool::connect_sqlite_app(&f.path, 1).await.unwrap();
            let (status, page) = json_request(
                app(Backend::Sqlite(fresh.clone()), &f.root),
                "GET",
                path,
                None,
                Some(&f.cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(page["items"], json!([created]));
            let(event_payload,event_target,audit_payload):(String,String,String)=sqlx::query_as("SELECT e.payload,e.target_type,a.payload FROM events e JOIN audit_log a ON e.target_id=a.target_id AND e.verb=a.verb WHERE e.verb='comment.created' AND e.target_id=?1").bind(comment_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(event_target, "comment");
            assert_eq!(event_payload, audit_payload);
            let payload: Value = serde_json::from_str(&event_payload).unwrap();
            let mut expected = vec![f.actor.to_string(), mentioned.to_string()];
            expected.sort();
            assert_eq!(payload["mentionedUserIds"], json!(expected));
            assert_eq!(payload["mentionedGroupIds"], json!([group]));
            let comment_path = f.comment_path(&created);
            let (status, updated) = f
                .request("PATCH", &comment_path, Some(json!({"body":"수정 中 😀"})))
                .await;
            assert_eq!(status, StatusCode::OK, "{updated}");
            assert_eq!(updated["body"], "수정 中 😀");
            let stored: String = sqlx::query_scalar("SELECT body FROM comments WHERE id=?1")
                .bind(comment_id.as_bytes().as_slice())
                .fetch_one(&fresh)
                .await
                .unwrap();
            assert_eq!(stored, "수정 中 😀");
            let before = effects(&f.pool).await;
            assert_eq!(
                f.request("PATCH", &comment_path, Some(json!({}))).await.0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                f.request("PATCH", &comment_path, Some(json!({"body":"수정 中 😀"})))
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(effects(&f.pool).await, before);
            let (status, reply) = f
                .request(
                    "POST",
                    path,
                    Some(json!({"body":"reply","parentId":comment_id})),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{reply}");
            assert_eq!(reply["parentId"], created["id"]);
            for suffix in ["resolve", "unresolve"] {
                assert_eq!(
                    f.request(
                        "POST",
                        &format!("{}/{suffix}", f.comment_path(&reply)),
                        None
                    )
                    .await
                    .0,
                    StatusCode::BAD_REQUEST
                );
            }
            let (status, page) = f.request("GET", &format!("{path}?limit=1"), None).await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(page["items"], json!([updated]));
            let (status, last) = f
                .request(
                    "GET",
                    &format!(
                        "{path}?limit=1&cursor={}",
                        page["nextCursor"].as_str().unwrap()
                    ),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{last}");
            assert_eq!(last["items"], json!([reply]));
            assert!(last["nextCursor"].is_null());
            assert_eq!(
                f.request(
                    "POST",
                    &format!("{comment_path}/reactions"),
                    Some(json!({"emoji":"👍","on":true}))
                )
                .await
                .0,
                StatusCode::OK
            );
            let events = effects(&f.pool).await.1;
            let (status, reaction) = f
                .request(
                    "POST",
                    &format!("{comment_path}/reactions"),
                    Some(json!({"emoji":"👍","on":true})),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(reaction["reactions"]["👍"]["count"], 1);
            assert_eq!(reaction["reactions"]["👍"]["reactedByMe"], true);
            assert_eq!(effects(&f.pool).await.1, events);
            let (status, resolved) = f
                .request("POST", &format!("{comment_path}/resolve"), None)
                .await;
            assert_eq!(status, StatusCode::OK);
            assert!(resolved["resolvedAt"].is_string());
            let (status, page) = json_request(
                app(Backend::Sqlite(fresh.clone()), &f.root),
                "GET",
                path,
                None,
                Some(&f.cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(page["items"][0], resolved);
            assert_eq!(page["items"][1], reply);
            let (status, unresolved) = f
                .request("POST", &format!("{comment_path}/unresolve"), None)
                .await;
            assert_eq!(status, StatusCode::OK);
            assert!(unresolved["resolvedAt"].is_null());
            let stored: Option<i64> =
                sqlx::query_scalar("SELECT resolved_at FROM comments WHERE id=?1")
                    .bind(comment_id.as_bytes().as_slice())
                    .fetch_one(&fresh)
                    .await
                    .unwrap();
            assert!(stored.is_none());
            let before = effects(&f.pool).await;
            assert_eq!(
                f.request("POST", &format!("{comment_path}/unresolve"), None)
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(effects(&f.pool).await, before);
            let (status, off) = f
                .request(
                    "POST",
                    &format!("{comment_path}/reactions"),
                    Some(json!({"emoji":"👍","on":false})),
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(off["reactions"], json!({}));
            let stored: String = sqlx::query_scalar("SELECT reactions FROM comments WHERE id=?1")
                .bind(comment_id.as_bytes().as_slice())
                .fetch_one(&fresh)
                .await
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), json!({}));
            let update_targets:Vec<String>=sqlx::query_scalar("SELECT target_type FROM events WHERE verb='comment.updated' AND json_extract(payload,'$.commentId')=?1").bind(comment_id.to_string()).fetch_all(&f.pool).await.unwrap();
            assert!(!update_targets.is_empty());
            assert!(update_targets
                .iter()
                .all(|target| target == if index == 2 { "task" } else { "document" }));
            assert_eq!(
                f.request("DELETE", &comment_path, None).await.0,
                StatusCode::OK
            );
            assert_eq!(
                f.request("DELETE", &comment_path, None).await.0,
                StatusCode::NOT_FOUND
            );
            let remaining: i64 =
                sqlx::query_scalar("SELECT count(*) FROM comments WHERE id=?1 OR parent_id=?1")
                    .bind(comment_id.as_bytes().as_slice())
                    .fetch_one(&fresh)
                    .await
                    .unwrap();
            assert_eq!(remaining, 0);
            fresh.close().await;
        }
        f.close().await;
    }

    async fn effects(pool: &sqlx::SqlitePool) -> (String, i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT coalesce(json_group_array(json_object('id',hex(id),'body',body,'chosung',chosung,'resolved',resolved_at,'reactions',reactions,'updated',updated_at)),'[]') FROM (SELECT * FROM comments ORDER BY id)),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log),(SELECT last_seq FROM event_sequence WHERE id=1),(SELECT count(*) FROM outbox_consumers),(SELECT count(*) FROM outbox_failures)").fetch_one(pool).await.unwrap()
    }

    async fn user(f: &Fixture, role: Option<&str>) -> (Uuid, Uuid, String) {
        let id = Uuid::now_v7();
        let credential = Uuid::now_v7();
        let token = fvoci_server::auth::token::new_token();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other')")
            .bind(id.as_bytes().as_slice())
            .bind(format!("{id}@comments.invalid"))
            .execute(&f.pool)
            .await
            .unwrap();
        if let Some(role) = role {
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(id.as_bytes().as_slice())
                .bind(role)
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,(unixepoch()+3600)*1000000)").bind(credential.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).bind(token.hash).execute(&f.pool).await.unwrap();
        (id, credential, token.token)
    }

    fn mutations(base: &str) -> Vec<(&'static str, String, Option<Value>)> {
        vec![
            ("PATCH", base.into(), Some(json!({"body":"denied change"}))),
            ("DELETE", base.into(), None),
            ("POST", format!("{base}/resolve"), None),
            ("POST", format!("{base}/unresolve"), None),
            (
                "POST",
                format!("{base}/reactions"),
                Some(json!({"emoji":"👍","on":true})),
            ),
        ]
    }

    #[tokio::test]
    async fn sqlite_comments_permissions_tenants_current_credentials_and_pat_scopes() {
        let f = Fixture::new().await;
        let (outsider, _, outsider_cookie) = user(&f, None).await;
        let (guest, _, guest_cookie) = user(&f, Some("guest")).await;
        let (member, _, member_cookie) = user(&f, Some("member")).await;
        let other_workspace = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO workspaces(id,slug,name) VALUES(?1,'other-comment-tenant','Other')",
        )
        .bind(other_workspace.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(other_workspace.as_bytes().as_slice())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut created = Vec::new();
        for path in f.paths() {
            let (status, row) = f
                .request("POST", &path, Some(json!({"body":"owned"})))
                .await;
            assert_eq!(status, StatusCode::CREATED, "{row}");
            created.push(row);
        }
        let before = effects(&f.pool).await;
        for (index, path) in f.paths().iter().enumerate() {
            for cookie in [&outsider_cookie, &guest_cookie] {
                for (method, url, body) in [
                    ("GET", path.clone(), None),
                    ("POST", path.clone(), Some(json!({"body":"denied"}))),
                ] {
                    let (status, error) =
                        json_request(f.app.clone(), method, &url, body, Some(cookie)).await;
                    assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
                }
                for (method, url, body) in mutations(&f.comment_path(&created[index])) {
                    assert_eq!(
                        json_request(f.app.clone(), method, &url, body, Some(cookie))
                            .await
                            .0,
                        StatusCode::NOT_FOUND
                    );
                }
            }
            let foreign = path.replace(&f.workspace.to_string(), &other_workspace.to_string());
            for (method, url, body) in [
                ("GET", foreign.clone(), None),
                ("POST", foreign, Some(json!({"body":"cross tenant"}))),
            ] {
                assert_eq!(f.request(method, &url, body).await.0, StatusCode::NOT_FOUND);
            }
            for (method, url, body) in mutations(
                &f.comment_path(&created[index])
                    .replace(&f.workspace.to_string(), &other_workspace.to_string()),
            ) {
                assert_eq!(f.request(method, &url, body).await.0, StatusCode::NOT_FOUND);
            }
            let token = fvoci_server::auth::token::new_token();
            let token_id = Uuid::now_v7();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Comments read','[\"documents.read\",\"tasks.read\"]')").bind(token_id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).bind(&token.hash).execute(&f.pool).await.unwrap();
            let headers = [("authorization", format!("Bearer {}", token.token))];
            let header_refs = [(headers[0].0, headers[0].1.as_str())];
            assert_eq!(
                http_request(f.app.clone(), "GET", path, None, None, None, &header_refs)
                    .await
                    .0,
                StatusCode::OK
            );
            for (method, url, body) in mutations(&f.comment_path(&created[index])) {
                assert_eq!(
                    http_request(
                        f.app.clone(),
                        method,
                        &url,
                        body.map(|b| b.to_string().into_bytes()),
                        Some("application/json"),
                        None,
                        &header_refs
                    )
                    .await
                    .0,
                    StatusCode::NOT_FOUND
                );
            }
            // Parent kind chooses the required write scope, even with a live PAT.
            let wrong = if index == 2 {
                "[\"documents.write\"]"
            } else {
                "[\"tasks.write\"]"
            };
            sqlx::query("UPDATE api_tokens SET scopes=?1 WHERE id=?2")
                .bind(wrong)
                .bind(token_id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                http_request(
                    f.app.clone(),
                    "PATCH",
                    &f.comment_path(&created[index]),
                    Some(b"{\"body\":\"wrong scope\"}".to_vec()),
                    Some("application/json"),
                    None,
                    &header_refs
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                http_request(
                    f.app.clone(),
                    "GET",
                    &path.replace(&f.workspace.to_string(), &other_workspace.to_string()),
                    None,
                    None,
                    None,
                    &header_refs
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(effects(&f.pool).await, before);
        // A non-author member edits at Edit but cannot purge at Manage.
        let first = f.comment_path(&created[0]);
        assert_eq!(
            json_request(f.app.clone(), "DELETE", &first, None, Some(&member_cookie))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            json_request(
                f.app.clone(),
                "PATCH",
                &first,
                Some(json!({"body":"member edit"})),
                Some(&member_cookie)
            )
            .await
            .0,
            StatusCode::OK
        );
        // Grant a guest View through a group, then remove it: current grants apply.
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Viewers')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(guest.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        for index in [1, 2] {
            let path = &f.paths()[index];
            assert_eq!(
                json_request(f.app.clone(), "GET", path, None, Some(&guest_cookie))
                    .await
                    .0,
                StatusCode::OK
            );
            assert_eq!(
                json_request(
                    f.app.clone(),
                    "POST",
                    path,
                    Some(json!({"body":"viewer"})),
                    Some(&guest_cookie)
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                json_request(
                    f.app.clone(),
                    "POST",
                    &format!("{}/reactions", f.comment_path(&created[index])),
                    Some(json!({"emoji":"👍","on":true})),
                    Some(&guest_cookie)
                )
                .await
                .0,
                StatusCode::OK
            );
        }
        sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(guest.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let current = effects(&f.pool).await;
        for index in [1, 2] {
            for (method, url, body) in mutations(&f.comment_path(&created[index])) {
                assert_eq!(
                    json_request(f.app.clone(), method, &url, body, Some(&guest_cookie))
                        .await
                        .0,
                    StatusCode::NOT_FOUND
                );
            }
        }
        assert_eq!(effects(&f.pool).await, current);
        for (sql, restore) in [
            (
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                "UPDATE sessions SET revoked_at=NULL WHERE id=?1",
            ),
            (
                "UPDATE sessions SET expires_at=1 WHERE id=?1",
                "UPDATE sessions SET expires_at=(unixepoch()+3600)*1000000 WHERE id=?1",
            ),
            (
                "UPDATE users SET suspended_at=1 WHERE id=?1",
                "UPDATE users SET suspended_at=NULL WHERE id=?1",
            ),
            (
                "UPDATE users SET deleted_at=1 WHERE id=?1",
                "UPDATE users SET deleted_at=NULL WHERE id=?1",
            ),
        ] {
            let id = if sql.contains("users") {
                f.actor
            } else {
                f.session
            };
            sqlx::query(sql)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            for row in &created {
                for (method, url, body) in mutations(&f.comment_path(row)) {
                    assert_eq!(
                        f.request(method, &url, body).await.0,
                        StatusCode::UNAUTHORIZED
                    );
                }
            }
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        assert_eq!(effects(&f.pool).await, current);
        // A correctly scoped token reaches every mutation, and revocation is current.
        let pat = fvoci_server::auth::token::new_token();
        let pat_id = Uuid::now_v7();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Write','[\"documents.write\",\"tasks.write\"]')").bind(pat_id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
        let bearer = format!("Bearer {}", pat.token);
        for (index, path) in f.paths().iter().enumerate() {
            let headers = [("authorization", bearer.as_str())];
            let (status, new, _) = http_request(
                f.app.clone(),
                "POST",
                path,
                Some(b"{\"body\":\"PAT write\"}".to_vec()),
                Some("application/json"),
                None,
                &headers,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{new}");
            for (method, url, body) in mutations(&f.comment_path(&created[index]))
                .into_iter()
                .filter(|(method, _, _)| *method != "DELETE")
            {
                assert_eq!(
                    http_request(
                        f.app.clone(),
                        method,
                        &url,
                        body.map(|b| b.to_string().into_bytes()),
                        Some("application/json"),
                        None,
                        &headers
                    )
                    .await
                    .0,
                    StatusCode::OK
                );
            }
            assert_eq!(
                http_request(
                    f.app.clone(),
                    "DELETE",
                    &f.comment_path(&new),
                    None,
                    None,
                    None,
                    &headers
                )
                .await
                .0,
                StatusCode::OK
            );
        }
        sqlx::query("DELETE FROM api_tokens WHERE id=?1")
            .bind(pat_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            http_request(
                f.app.clone(),
                "PATCH",
                &first,
                Some(b"{\"body\":\"revoked\"}".to_vec()),
                Some("application/json"),
                None,
                &[("authorization", bearer.as_str())]
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_ne!(outsider, member);
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_comments_validation_archives_trash_and_atomic_effects() {
        let f = Fixture::new().await;
        let mut roots = Vec::new();
        for path in f.paths() {
            let (status, row) = f.request("POST", &path, Some(json!({"body":"root"}))).await;
            assert_eq!(status, StatusCode::CREATED, "{row}");
            roots.push(row);
        }
        for (index, path) in f.paths().iter().enumerate() {
            let before = effects(&f.pool).await;
            for body in [
                json!({"body":"a\u{0}b"}),
                json!({"body":" "}),
                json!({"body":"😀".repeat(4001)}),
                json!({"body":"bad","unknown":true}),
                json!({"body":"bad","mentionedUserIds":(0..51).map(|_|Uuid::now_v7()).collect::<Vec<_>>()}),
                json!({"body":"bad","parentId":roots[(index+1)%3]["id"]}),
            ] {
                assert_eq!(
                    f.request("POST", path, Some(body)).await.0,
                    StatusCode::BAD_REQUEST
                );
            }
            assert_eq!(
                f.request(
                    "POST",
                    path,
                    Some(json!({"body":"missing parent","parentId":Uuid::now_v7()}))
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            for body in ["a\u{0}b".into(), " ".into(), "😀".repeat(4001)] {
                assert_eq!(
                    f.request(
                        "PATCH",
                        &f.comment_path(&roots[index]),
                        Some(json!({"body":body}))
                    )
                    .await
                    .0,
                    StatusCode::BAD_REQUEST
                );
            }
            assert_eq!(
                f.request(
                    "POST",
                    &format!("{}/reactions", f.comment_path(&roots[index])),
                    Some(json!({"emoji":"invalid","on":true}))
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            for suffix in ["?limit=0", "?limit=101", "?cursor=wrong"] {
                assert_eq!(
                    f.request("GET", &format!("{path}{suffix}"), None).await.0,
                    StatusCode::BAD_REQUEST
                );
            }
            assert_eq!(effects(&f.pool).await, before);
            let (status, _) = f
                .request("POST", path, Some(json!({"body":"😀".repeat(4000)})))
                .await;
            assert_eq!(status, StatusCode::CREATED);
        }
        // Archive errors are masked until permission succeeds, and reads remain available.
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(f.project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = effects(&f.pool).await;
        for index in [1, 2] {
            assert_eq!(
                f.request("GET", &f.paths()[index], None).await.0,
                StatusCode::OK
            );
            let (status, error) = f
                .request("POST", &f.paths()[index], Some(json!({"body":"archived"})))
                .await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(error["code"], "project_archived");
            for (method, url, body) in mutations(&f.comment_path(&roots[index])) {
                assert_eq!(f.request(method, &url, body).await.0, StatusCode::CONFLICT);
            }
        }
        assert_eq!(effects(&f.pool).await, before);
        sqlx::query("UPDATE projects SET status='active' WHERE id=?1")
            .bind(f.project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET archived_at=1 WHERE id=?1")
            .bind(f.task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (status, error) = f
            .request("POST", &f.paths()[2], Some(json!({"body":"archived"})))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["code"], "task_archived");
        for (method, url, body) in mutations(&f.comment_path(&roots[2])) {
            assert_eq!(f.request(method, &url, body).await.0, StatusCode::CONFLICT);
        }
        sqlx::query("UPDATE tasks SET archived_at=NULL WHERE id=?1")
            .bind(f.task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for (sql, restore, id, indexes) in [
            (
                "UPDATE workspaces SET deleted_at=1 WHERE id=?1",
                "UPDATE workspaces SET deleted_at=NULL WHERE id=?1",
                f.workspace,
                vec![0, 1, 2],
            ),
            (
                "UPDATE projects SET deleted_at=1 WHERE id=?1",
                "UPDATE projects SET deleted_at=NULL WHERE id=?1",
                f.project,
                vec![1, 2],
            ),
            (
                "UPDATE documents SET deleted_at=1 WHERE id=?1",
                "UPDATE documents SET deleted_at=NULL WHERE id=?1",
                f.document,
                vec![0],
            ),
            (
                "UPDATE documents SET project_id=?2 WHERE id=?1",
                "UPDATE documents SET project_id=NULL WHERE id=?1",
                f.document,
                vec![],
            ),
            (
                "UPDATE tasks SET deleted_at=1 WHERE id=?1",
                "UPDATE tasks SET deleted_at=NULL WHERE id=?1",
                f.task,
                vec![2],
            ),
        ] {
            if sql.contains("?2") {
                sqlx::query(sql)
                    .bind(id.as_bytes().as_slice())
                    .bind(f.project.as_bytes().as_slice())
                    .execute(&f.pool)
                    .await
                    .unwrap();
                assert_eq!(
                    f.request("GET", &f.paths()[0], None).await.0,
                    StatusCode::NOT_FOUND
                );
                assert_eq!(
                    f.request("POST", &f.paths()[0], Some(json!({"body":"moved"})))
                        .await
                        .0,
                    StatusCode::NOT_FOUND
                );
            } else {
                sqlx::query(sql)
                    .bind(id.as_bytes().as_slice())
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            let before = effects(&f.pool).await;
            for index in indexes {
                for (method, url, body) in mutations(&f.comment_path(&roots[index])) {
                    assert_eq!(f.request(method, &url, body).await.0, StatusCode::NOT_FOUND);
                }
                assert_eq!(
                    f.request("GET", &f.paths()[index], None).await.0,
                    StatusCode::NOT_FOUND
                );
                assert_eq!(
                    f.request("POST", &f.paths()[index], Some(json!({"body":"trashed"})))
                        .await
                        .0,
                    StatusCode::NOT_FOUND
                );
            }
            assert_eq!(effects(&f.pool).await, before);
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        // Include replies so a failed parent purge must restore cascaded rows too.
        for (index, path) in f.paths().iter().enumerate() {
            assert_eq!(
                f.request(
                    "POST",
                    path,
                    Some(json!({"body":"preserved reply","parentId":roots[index]["id"]}))
                )
                .await
                .0,
                StatusCode::CREATED
            );
        }
        // Every side-effecting verb rolls back its row, event, audit, sequence and outbox state.
        for row in &roots {
            assert_eq!(
                f.request("POST", &format!("{}/resolve", f.comment_path(row)), None)
                    .await
                    .0,
                StatusCode::OK
            );
        }
        sqlx::query("CREATE TRIGGER comments_fail_audit BEFORE INSERT ON audit_log BEGIN SELECT RAISE(ABORT,'injected comment audit failure'); END").execute(&f.pool).await.unwrap();
        for (index, path) in f.paths().iter().enumerate() {
            let before = effects(&f.pool).await;
            assert_eq!(
                f.request("POST", path, Some(json!({"body":"rollback create"})))
                    .await
                    .0,
                StatusCode::INTERNAL_SERVER_ERROR
            );
            assert_eq!(effects(&f.pool).await, before);
            for (method, url, body) in mutations(&f.comment_path(&roots[index])) {
                let (status, error) = f.request(method, &url, body).await;
                assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{error}");
                assert_eq!(effects(&f.pool).await, before);
            }
        }
        sqlx::query("DROP TRIGGER comments_fail_audit")
            .execute(&f.pool)
            .await
            .unwrap();
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_comments_writer_rechecks_revoked_credential_and_duplicate_deletes() {
        use fvoci_server::db::comments::{purge_comment_backend, CommentDbError};
        use std::future::Future;
        let f = Fixture::new().await;
        let (status, created) = f
            .request("POST", &f.paths()[2], Some(json!({"body":"serialized"})))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let comment = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let before = effects(&f.pool).await;
        let mut blocker = f.pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let backend = Backend::Sqlite(f.pool.clone());
        let mut deletion = Box::pin(purge_comment_backend(
            &backend,
            f.workspace,
            f.actor,
            f.session,
            comment,
            None,
        ));
        // Poll the product operation while another connection owns the writer.
        // No elapsed-time assertion: committing the revocation orders the outcome.
        futures_util::future::poll_fn(|cx| {
            assert!(deletion.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.session.as_bytes().as_slice())
            .execute(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("COMMIT").execute(&mut *blocker).await.unwrap();
        drop(blocker);
        assert!(matches!(
            deletion.await.unwrap(),
            Err(CommentDbError::NotFound)
        ));
        assert_eq!(effects(&f.pool).await, before);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(f.session.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            purge_comment_backend(&backend, f.workspace, f.actor, f.session, comment, None),
            purge_comment_backend(&backend, f.workspace, f.actor, f.session, comment, None)
        );
        let results = [a.unwrap(), b.unwrap()];
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err(CommentDbError::NotFound)))
                .count(),
            1
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE verb='comment.deleted'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        f.close().await;
    }
}

async fn json_request_origin(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    origin: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", origin);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json)
}

#[tokio::test]
async fn wiki_document_comment_create_list_resolve_and_react() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "댓글 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "첫 댓글"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    assert_eq!(created["body"], "첫 댓글");

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, resolved) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(resolved["resolvedAt"].is_string());

    let (status, reacted) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions"),
        Some(json!({"emoji": "👍", "on": true})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reacted["reactions"]["👍"]["count"], 1);
    assert_eq!(reacted["reactions"]["👍"]["reactedByMe"], true);

    harness.cleanup().await;
}

#[tokio::test]
async fn task_comment_create_and_reply_inherits_target() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (status, root) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "루트"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let root_id = root["id"].as_str().unwrap();

    let (status, reply) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "답글", "parentId": root_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reply["parentId"], root_id);
    assert_eq!(reply["taskId"], task_id);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}/resolve",
            reply["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    harness.cleanup().await;
}

#[tokio::test]
async fn guest_and_non_member_get_masked_not_found() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "비공개"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (_, guest_post) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "게스트"})),
        Some(&guest.cookie),
    )
    .await;
    let (status, missing) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/comments",
            Uuid::now_v7()
        ),
        Some(json!({"body": "없음"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(guest_post, missing);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn private_project_viewer_can_list_but_not_create_task_comment() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role": "viewer"})),
        Some(&member.cookie),
    )
    .await;
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "Lead task"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "멤버 댓글"})),
        Some(&member.cookie),
    )
    .await;

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "뷰어는 불가"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn mentioned_group_ids_expand_into_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "groupmate").await;
    let (status, group) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "랩팀"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group:?}");
    let group_id = group["id"].as_str().unwrap();
    let (status, added) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{added:?}");

    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "그룹 멘션"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({
            "body": "@랩팀",
            "mentionedGroupIds": [group_id]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM fvoci.events WHERE verb = 'comment.created' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&admin)
    .await
    .expect("comment.created payload");
    let mentioned_users = payload["mentionedUserIds"]
        .as_array()
        .expect("mentionedUserIds");
    assert!(
        mentioned_users
            .iter()
            .any(|id| id.as_str() == Some(&member.user_id.to_string())),
        "{payload:?}"
    );
    let mentioned_groups = payload["mentionedGroupIds"]
        .as_array()
        .expect("mentionedGroupIds");
    assert!(
        mentioned_groups
            .iter()
            .any(|id| id.as_str() == Some(group_id)),
        "{payload:?}"
    );

    let (status, unknown) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({
            "body": "없는 그룹",
            "mentionedGroupIds": [Uuid::now_v7().to_string()]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{unknown:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_create_audit_failure_rolls_back_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "감사"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_comment_audit_fail").await;
    let events_before = count_rows(&admin, "events").await;
    let comments_before = count_rows(&admin, "comments").await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "실패"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "events").await, events_before);
    assert_eq!(count_rows(&admin, "comments").await, comments_before);
    drop_insert_fail_trigger(&admin, "audit_log", "test_comment_audit_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_reaction_toggles_leave_single_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "반응"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "좋아요"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions");
    let body = json!({"emoji": "👍", "on": true});
    let (on_a, on_b) = tokio::join!(
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(body.clone()),
            Some(&cookie)
        ),
        json_request(app.clone(), "POST", &path, Some(body), Some(&cookie)),
    );
    assert_eq!(on_a.0, StatusCode::OK);
    assert_eq!(on_b.0, StatusCode::OK);
    assert_eq!(on_a.1["reactions"]["👍"]["count"], 1);
    assert_eq!(on_b.1["reactions"]["👍"]["count"], 1);

    let (status, off) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"emoji": "👍", "on": false})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(off["reactions"].as_object().unwrap().get("👍"), None);

    let admin = admin_pool(&harness).await;
    let reactions: (serde_json::Value,) =
        sqlx::query_as("SELECT reactions FROM fvoci.comments WHERE id = $1")
            .bind(Uuid::parse_str(comment_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    let ids = reactions
        .0
        .get("👍")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0);
    assert_eq!(ids, 0);
    let _ = user_id;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_comment_delete_is_author_or_manage() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin_db = admin_pool(&harness).await;
    let member = add_workspace_user(&admin_db, workspace_id, "member", "member").await;
    let admin = add_workspace_user(&admin_db, workspace_id, "admin", "admin").await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "권한"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, owner_comment) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "소유자"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let owner_comment_id = owner_comment["id"].as_str().unwrap();

    let (status, member_comment) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "멤버"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member_comment_id = member_comment["id"].as_str().unwrap();

    let (status, extra_member) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "멤버 본인 삭제"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}",
            extra_member["id"].as_str().unwrap()
        ),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, denied) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{owner_comment_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(denied["code"], "not_found");

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{owner_comment_id}"),
        Some(json!({"body": "멤버가 수정"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["body"], "멤버가 수정");

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{member_comment_id}"),
        None,
        Some(&admin.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, owner_again) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "소유자가 지움"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}",
            owner_again["id"].as_str().unwrap()
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin_db.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_body_limit_counts_utf16_code_units() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "길이"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "가".repeat(8000)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, over_korean) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "가".repeat(8001)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(over_korean["code"], "invalid_input");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "👍".repeat(4000)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, over_emoji) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "👍".repeat(4001)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(over_emoji["code"], "invalid_input");

    harness.cleanup().await;
}

#[tokio::test]
async fn resolve_and_unresolve_reject_cross_origin() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "출처"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "원본"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    let resolve = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve");
    let unresolve = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve");

    let (status, evil) = json_request_origin(
        app.clone(),
        "POST",
        &resolve,
        None,
        Some(&cookie),
        "https://evil.example",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(evil["code"], "origin_mismatch");

    let (status, _) = json_request(app.clone(), "POST", &resolve, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, evil_unresolve) = json_request_origin(
        app.clone(),
        "POST",
        &unresolve,
        None,
        Some(&cookie),
        "https://evil.example",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(evil_unresolve["code"], "origin_mismatch");

    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_comments_use_project_permission() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let document_id = lab["rootDocumentId"]
        .as_str()
        .expect("project root document");
    let comments = format!(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments"
    );

    let (status, listed) = json_request(app.clone(), "GET", &comments, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "프로젝트 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["body"], "프로젝트 문서");
    assert_eq!(created["documentId"], document_id);

    let (status, wiki) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(wiki["code"], "not_found");

    let hid = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let private_project_id = hid["id"].as_str().unwrap();
    let private_document_id = hid["rootDocumentId"].as_str().expect("private root");
    let private_comments = format!(
        "/api/v1/workspaces/{workspace_id}/projects/{private_project_id}/documents/{private_document_id}/comments"
    );
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    let (status, denied) = json_request(
        app.clone(),
        "GET",
        &private_comments,
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{private_project_id}/members"),
        Some(json!({"userId": outsider.user_id.to_string(), "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    let (status, viewer_list) = json_request(
        app.clone(),
        "GET",
        &private_comments,
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{viewer_list:?}");
    let (status, viewer_create) = json_request(
        app.clone(),
        "POST",
        &private_comments,
        Some(json!({"body": "뷰어는 불가"})),
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{viewer_create:?}");

    admin.close().await;
    harness.cleanup().await;
}

/// A wiki comment create whose unlocked reads saw a wiki document, but whose
/// document row lock returns it already moved into a project, is refused
/// instead of locking the project after the document (collab and project
/// writes lock project -> document; the inverse order could deadlock).
#[tokio::test]
async fn wiki_comment_refuses_document_moved_into_project_meanwhile() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "이동 중 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc:?}");
    let document_id = Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap();
    let lab = create_project(app.clone(), &cookie, workspace_id, "MOV", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();

    // The mover holds the document row with the project affiliation uncommitted.
    let mut mover = admin.begin().await.unwrap();
    let mover_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *mover)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET project_id = $2, number = 99999 WHERE id = $1")
        .bind(document_id)
        .bind(project_id)
        .execute(&mut *mover)
        .await
        .unwrap();

    let create = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
                Some(json!({"body": "경합 댓글"})),
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_query_blocked_by(&admin, mover_pid, "%fvoci.documents%").await;
    mover.commit().await.unwrap();

    let (status, body) = create.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    let comments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.comments WHERE document_id = $1")
            .bind(document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(comments, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_pat_scopes_match_parent_kind() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "PAT 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, read_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "doc-read", "scopes": ["documents.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{read_token:?}");
    let read_secret = read_token["token"].as_str().unwrap().to_string();
    let read_auth = format!("Bearer {read_secret}");
    let (status, listed, _) = http_request(
        app.clone(),
        "GET",
        &comments,
        None,
        None,
        None,
        &[("authorization", read_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let (status, denied, _) = http_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "읽기만"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", read_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    let (status, write_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "doc-write", "scopes": ["documents.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{write_token:?}");
    let write_secret = write_token["token"].as_str().unwrap().to_string();
    let write_auth = format!("Bearer {write_secret}");
    let (status, created, _) = http_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "쓰기"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", write_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let lab = create_project(app.clone(), &cookie, workspace_id, "PAT", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "PAT 태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let task_comments = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments");
    let (status, task_denied, _) = http_request(
        app.clone(),
        "POST",
        &task_comments,
        Some(json!({"body": "문서 토큰"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", write_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{task_denied:?}");

    let (status, task_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "task-write", "scopes": ["tasks.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_token:?}");
    let task_secret = task_token["token"].as_str().unwrap().to_string();
    let task_auth = format!("Bearer {task_secret}");
    let (status, task_created, _) = http_request(
        app.clone(),
        "POST",
        &task_comments,
        Some(json!({"body": "태스크"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", task_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_created:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn archived_parent_comment_writes_return_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "보관"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments");

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관 전"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"archived": true})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, archived_task) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관된 태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(archived_task["code"], "task_archived");

    sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1")
        .bind(Uuid::parse_str(task_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let (status, listed) = json_request(app.clone(), "GET", &comments, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, archived_project) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관된 프로젝트"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(archived_project["code"], "project_archived");

    let _ = created;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_create_unknown_fields_and_empty_group_ids() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "필드"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, extra) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "a", "extra": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(extra["code"], "invalid_input");

    let (status, empty_groups) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "빈 그룹", "mentionedGroupIds": []})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(empty_groups["body"], "빈 그룹");

    let (status, limit_zero) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?limit=0"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(limit_zero["code"], "invalid_input");

    let (status, limit_high) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?limit=101"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(limit_high["code"], "invalid_input");

    harness.cleanup().await;
}

async fn begin_row_blocker(
    admin: &sqlx::PgPool,
    sql: &'static str,
    id: Uuid,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
    let mut blocker = admin.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query(sql)
        .bind(id)
        .execute(&mut *blocker)
        .await
        .unwrap();
    (blocker, pid)
}

async fn comment_verb_count(
    admin: &sqlx::PgPool,
    table: &str,
    verb: &str,
    comment_id: Uuid,
) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM fvoci.{table} WHERE verb = $1 AND payload->>'commentId' = $2"
    )))
    .bind(verb)
    .bind(comment_id.to_string())
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn wiki_comment_fixture(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    body: &str,
) -> (Uuid, Uuid) {
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "경합 문서"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc:?}");
    let document_id = Uuid::parse_str(doc["id"].as_str().unwrap()).unwrap();
    let (status, created) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": body})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    (
        document_id,
        Uuid::parse_str(created["id"].as_str().unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn task_comment_refuses_task_trashed_while_waiting_for_project_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "commenter").await;
    let lab = create_project(app.clone(), &owner_cookie, workspace_id, "TRS", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "휴지통 경합"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();

    // Hold the task row so the real trash request stops after taking the
    // project lock, before it marks the task deleted.
    let (blocker, blocker_pid) = begin_row_blocker(
        &admin,
        "SELECT 1 FROM fvoci.tasks WHERE id = $1 FOR SHARE",
        task_id,
    )
    .await;
    let trash = tokio::spawn({
        let app = app.clone();
        let cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash"),
                None,
                Some(&cookie),
            )
            .await
        }
    });
    let trash_pid = wait_for_query_blocked_by(&admin, blocker_pid, "%fvoci.tasks%").await;

    // The comment reads the still-live task, then waits for the trash's project lock.
    let create = tokio::spawn({
        let app = app.clone();
        let cookie = member.cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
                Some(json!({"body": "휴지통 태스크 댓글"})),
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_query_blocked_by(&admin, trash_pid, "%fvoci.projects%").await;
    blocker.commit().await.unwrap();

    let (trash_status, trash_body) = trash.await.unwrap();
    assert!(trash_status.is_success(), "{trash_status} {trash_body:?}");
    let (status, body) = create.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    let comments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.comments WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(comments, 0);
    let created_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'comment.created' AND payload->>'taskId' = $1",
    )
    .bind(task_id.to_string())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(created_events, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_comment_deletes_record_one_delete() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "author").await;
    let (_, comment_id) =
        wiki_comment_fixture(app.clone(), &member.cookie, workspace_id, "동시 삭제").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}");

    // The author's delete stops on the comment row while it holds the document lock.
    let (blocker, blocker_pid) = begin_row_blocker(
        &admin,
        "SELECT 1 FROM fvoci.comments WHERE id = $1 FOR UPDATE",
        comment_id,
    )
    .await;
    let author_delete = tokio::spawn({
        let app = app.clone();
        let cookie = member.cookie.clone();
        let path = path.clone();
        async move { json_request(app, "DELETE", &path, None, Some(&cookie)).await }
    });
    // Blocked on the comment row: the DELETE itself, or the row lock taken before it.
    let author_pid = wait_for_query_blocked_by(&admin, blocker_pid, "%fvoci.comments%").await;
    // The owner's delete has already read the comment and waits on that document lock.
    let owner_delete = tokio::spawn({
        let app = app.clone();
        let cookie = owner_cookie.clone();
        let path = path.clone();
        async move { json_request(app, "DELETE", &path, None, Some(&cookie)).await }
    });
    wait_for_query_blocked_by(&admin, author_pid, "%fvoci.documents%").await;
    blocker.commit().await.unwrap();

    let (author_status, author_body) = author_delete.await.unwrap();
    assert_eq!(author_status, StatusCode::OK, "{author_body:?}");
    let (owner_status, owner_body) = owner_delete.await.unwrap();
    assert_eq!(owner_status, StatusCode::NOT_FOUND, "{owner_body:?}");
    assert_eq!(
        comment_verb_count(&admin, "events", "comment.deleted", comment_id).await,
        1
    );
    assert_eq!(
        comment_verb_count(&admin, "audit_log", "comment.deleted", comment_id).await,
        1
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn unresolve_waiting_behind_resolve_applies_to_resolved_comment() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "resolver").await;
    let (_, comment_id) =
        wiki_comment_fixture(app.clone(), &owner_cookie, workspace_id, "해결 경합").await;
    let base = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}");

    let (blocker, blocker_pid) = begin_row_blocker(
        &admin,
        "SELECT 1 FROM fvoci.comments WHERE id = $1 FOR UPDATE",
        comment_id,
    )
    .await;
    let resolve = tokio::spawn({
        let app = app.clone();
        let cookie = member.cookie.clone();
        let path = format!("{base}/resolve");
        async move { json_request(app, "POST", &path, None, Some(&cookie)).await }
    });
    let resolver_pid =
        wait_for_query_blocked_by(&admin, blocker_pid, "%UPDATE fvoci.comments%").await;
    // The unresolve read the comment as unresolved and waits on the resolver's document lock.
    let unresolve = tokio::spawn({
        let app = app.clone();
        let cookie = owner_cookie.clone();
        let path = format!("{base}/unresolve");
        async move { json_request(app, "POST", &path, None, Some(&cookie)).await }
    });
    wait_for_query_blocked_by(&admin, resolver_pid, "%fvoci.documents%").await;
    blocker.commit().await.unwrap();

    let (status, resolved) = resolve.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{resolved:?}");
    assert!(resolved["resolvedAt"].is_string());
    let (status, unresolved) = unresolve.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{unresolved:?}");
    assert!(unresolved["resolvedAt"].is_null(), "{unresolved:?}");
    let resolved_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT resolved_at FROM fvoci.comments WHERE id = $1")
            .bind(comment_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(resolved_at, None);
    assert_eq!(
        comment_verb_count(&admin, "events", "comment.resolved", comment_id).await,
        1
    );
    assert_eq!(
        comment_verb_count(&admin, "events", "comment.updated", comment_id).await,
        1
    );

    admin.close().await;
    harness.cleanup().await;
}

/// A database failure inside a comment's parent permission check reaches the
/// route as a logged 500 `internal_error`. Before, the helpers mapped every
/// sqlx error to `not_found`, so a transient fault looked like a deleted or
/// hidden parent.
#[tokio::test]
async fn comment_permission_query_failure_is_internal_error() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "CERR", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let root_id = project["rootDocumentId"].as_str().unwrap().to_string();
    let ws = format!("/api/v1/workspaces/{workspace_id}");
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("{ws}/projects/{project_id}/tasks"),
        Some(json!({"title": "Comment fault task"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    let task_id = task["id"].as_str().unwrap().to_string();
    let paths = [
        format!("{ws}/tasks/{task_id}/comments"),
        format!("{ws}/projects/{project_id}/documents/{root_id}/comments"),
    ];
    for path in &paths {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            path,
            Some(json!({"body": "before"})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "POST {path}: {body:?}");
    }

    let app_pool = project_harness::app_pool(&harness).await;
    let app_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&app_pool)
        .await
        .unwrap();
    app_pool.close().await;
    let role = format!("\"{}\"", app_role.replace('"', "\"\""));
    // The project permission check reads project_members after the parent
    // lookup; nothing earlier on these paths does.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "REVOKE SELECT ON fvoci.project_members FROM {role}"
    )))
    .execute(&admin)
    .await
    .unwrap();
    let comments_before = count_rows(&admin, "comments").await;
    for path in &paths {
        let (status, body) = json_request(app.clone(), "GET", path, None, Some(&cookie)).await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "GET {path}: {body:?}"
        );
        assert_eq!(body["code"], "internal_error", "GET {path}: {body:?}");
        let (status, body) = json_request(
            app.clone(),
            "POST",
            path,
            Some(json!({"body": "lost"})),
            Some(&cookie),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "POST {path}: {body:?}"
        );
        assert_eq!(body["code"], "internal_error", "POST {path}: {body:?}");
    }
    assert_eq!(count_rows(&admin, "comments").await, comments_before);

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "GRANT SELECT ON fvoci.project_members TO {role}"
    )))
    .execute(&admin)
    .await
    .unwrap();
    for path in &paths {
        let (status, body) = json_request(app.clone(), "GET", path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body:?}");
        assert_eq!(body["items"].as_array().map(Vec::len), Some(1), "{body:?}");
    }
    admin.close().await;
    harness.cleanup().await;
}
