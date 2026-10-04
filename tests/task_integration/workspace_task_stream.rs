//! C6: the workspace task stream (`/task-stream`) replaces one persistent
//! response per project with one per workspace. Every poll re-evaluates the
//! credential, the membership and View on each hinted project in its own
//! transaction (nothing cached from admission), under the restricted app role.

use super::*;

/// One admitted SSE body, read incrementally.
struct SseReader {
    body: axum::body::BodyDataStream,
    buf: Vec<u8>,
}

impl SseReader {
    /// Reads until `needle` appears; false on timeout or end of body.
    async fn until(&mut self, needle: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if contains(&self.buf, needle) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match timeout(left, self.body.next()).await {
                Ok(Some(Ok(bytes))) => self.buf.extend_from_slice(&bytes),
                _ => return false,
            }
        }
    }

    /// True once the server ends the body within `within`.
    async fn ends(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match timeout(left, self.body.next()).await {
                Ok(Some(Ok(bytes))) => self.buf.extend_from_slice(&bytes),
                Ok(Some(Err(_))) | Ok(None) => return true,
                Err(_) => return false,
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).into_owned()
    }
}

fn contains(buf: &[u8], needle: &str) -> bool {
    buf.windows(needle.len()).any(|w| w == needle.as_bytes())
}

async fn open_stream(
    app: &axum::Router,
    workspace_id: Uuid,
    cookie: &str,
) -> Result<SseReader, StatusCode> {
    let mut request = Request::builder()
        .method("GET")
        .uri(format!("/api/v1/workspaces/{workspace_id}/task-stream"))
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("origin", "http://localhost")
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(test_peer()));
    let response = app.clone().oneshot(request).await.expect("sse response");
    if response.status() != StatusCode::OK {
        return Err(response.status());
    }
    Ok(SseReader {
        body: response.into_body().into_data_stream(),
        buf: Vec::new(),
    })
}

async fn create_task(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
) -> String {
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "c6"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    task["id"].as_str().expect("task id").to_string()
}

fn hint(verb: &str, task_id: &str, project_id: &str) -> String {
    json!({"verb": verb, "taskId": task_id, "projectId": project_id}).to_string()
}

const WAIT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn workspace_task_stream_hints_only_projects_the_actor_can_view_now() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "c6-viewer").await;
    let open = create_project(app.clone(), &owner, workspace_id, "WOPEN", "workspace").await;
    let hidden = create_project(app.clone(), &owner, workspace_id, "WHIDE", "private").await;
    let shared = create_project(app.clone(), &owner, workspace_id, "WSHARE", "private").await;
    let (open, hidden, shared) = (
        open["id"].as_str().unwrap().to_string(),
        hidden["id"].as_str().unwrap().to_string(),
        shared["id"].as_str().unwrap().to_string(),
    );
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{shared}/members"),
        Some(json!({"userId": viewer.user_id.to_string(), "role": "viewer"})),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let mut stream = open_stream(&app, workspace_id, &viewer.cookie)
        .await
        .expect("a member is admitted");
    assert!(stream.until("event: open", WAIT).await, "open first");

    // Committed in this order, so the hidden events precede the marker.
    let hidden_task = create_task(&app, &owner, workspace_id, &hidden).await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{hidden_task}/comments"),
        Some(json!({"body": "hidden"})),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let shared_task = create_task(&app, &owner, workspace_id, &shared).await;
    let open_task = create_task(&app, &owner, workspace_id, &open).await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{open_task}/comments"),
        Some(json!({"body": "visible"})),
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let activity = hint("task.activity", &open_task, &open);
    assert!(stream.until(&activity, WAIT).await, "{}", stream.text());
    let body = stream.text();
    assert!(body.contains(&hint("task.created", &shared_task, &shared)));
    assert!(body.contains(&hint("task.created", &open_task, &open)));
    assert!(!body.contains(&hidden_task), "hidden task hinted: {body}");
    assert!(!body.contains(&hidden), "hidden project named: {body}");

    // Lose View on one project: its hints stop on the next poll while the
    // stream keeps serving the others (no admission-time ACL).
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{shared}/members/{}",
            viewer.user_id
        ),
        None,
        Some(&owner),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let revoked_task = create_task(&app, &owner, workspace_id, &shared).await;
    let marker = create_task(&app, &owner, workspace_id, &open).await;
    assert!(
        stream
            .until(&hint("task.created", &marker, &open), WAIT)
            .await,
        "{}",
        stream.text()
    );
    assert!(
        !stream.text().contains(&revoked_task),
        "hint after project revocation: {}",
        stream.text()
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_task_stream_ends_on_session_revoke_or_membership_loss() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let leaving = add_workspace_user(&admin, workspace_id, "member", "c6-leaving").await;
    let session = add_workspace_user(&admin, workspace_id, "member", "c6-session").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "c6-outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .expect("fixture: outsider is no member");
    assert_eq!(
        open_stream(&app, workspace_id, &outsider.cookie)
            .await
            .err(),
        Some(StatusCode::NOT_FOUND),
        "a non-member is refused like the task list"
    );

    let mut by_member = open_stream(&app, workspace_id, &leaving.cookie)
        .await
        .expect("member admitted");
    let mut by_session = open_stream(&app, workspace_id, &session.cookie)
        .await
        .expect("member admitted");
    assert!(by_member.until("event: open", WAIT).await);
    assert!(by_session.until("event: open", WAIT).await);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            leaving.user_id
        ),
        None,
        Some(&owner),
    )
    .await;
    assert!(status.is_success(), "remove member: {status}");
    assert!(
        by_member.ends(WAIT).await,
        "membership loss ends the stream"
    );

    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&session.cookie),
    )
    .await;
    assert!(status.is_success(), "logout: {status}");
    assert!(
        by_session.ends(WAIT).await,
        "a revoked session ends the stream"
    );
    assert_eq!(
        open_stream(&app, workspace_id, &session.cookie).await.err(),
        Some(StatusCode::UNAUTHORIZED)
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_task_stream_holds_one_stream_guard_per_connection() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id, hub) = setup_session_with_hub(&harness).await;
    for key in ["GA", "GB", "GC"] {
        create_project(app.clone(), &cookie, workspace_id, key, "workspace").await;
    }
    let mut held = Vec::new();
    for _ in 0..64 {
        let mut stream = open_stream(&app, workspace_id, &cookie)
            .await
            .expect("under the cap");
        assert!(stream.until("event: open", WAIT).await);
        held.push(stream);
    }
    assert!(
        wait_for_hub_active(&hub, 64, Duration::from_secs(3)).await,
        "one guard per connection regardless of project count, saw {}",
        hub.active_count()
    );
    assert_eq!(
        open_stream(&app, workspace_id, &cookie).await.err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
    drop(held);
    assert!(wait_for_hub_active(&hub, 0, Duration::from_secs(5)).await);
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_task_stream_overflow_ends_and_reconnect_resyncs() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &owner, workspace_id, "WFLOOD", "workspace").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let mut stream = open_stream(&app, workspace_id, &owner)
        .await
        .expect("admitted");
    assert!(stream.until("event: open", WAIT).await);

    // One committed transaction: every event lands in one poll page, which
    // overflows the bounded queue while the body is not being read.
    let flood = 3 * fvoci_server::streams::STREAM_CHANNEL_CAPACITY;
    let mut tx = admin.begin().await.expect("tx");
    for _ in 0..flood {
        let task_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, actor_user_id, payload, channel)
            VALUES ($1, $2, 'task.created', 'task', $3, $4, $5::jsonb, 'web')
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(task_id)
        .bind(owner_id)
        .bind(json!({"taskId": task_id.to_string(), "projectId": project_id.to_string()}))
        .execute(&mut *tx)
        .await
        .expect("insert task event");
    }
    tx.commit().await.expect("commit");

    assert!(stream.ends(WAIT).await, "a full queue ends the stream");
    let delivered = stream.text().matches("event: task").count();
    assert!(
        delivered < flood,
        "overflow must not deliver the whole flood ({delivered} of {flood})"
    );
    let mut again = open_stream(&app, workspace_id, &owner)
        .await
        .expect("reconnect admitted");
    assert!(
        again.until("event: open", WAIT).await,
        "the reconnect starts with open, the client's resync"
    );
    admin.close().await;
    harness.cleanup().await;
}
