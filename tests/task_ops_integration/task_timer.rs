mod task_timer {
    // First timer consumer: real HTTP boundary, committed intervals, fresh read.
    use super::*;

    #[tokio::test]
    async fn timer_start_pause_fresh_client_resume_stop_commits_without_completing_task() {
        let harness = TestDb::bootstrap().await;
        let app_role = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&harness.app_url)
            .await
            .unwrap();
        let flags: (bool, bool) = sqlx::query_as(
            "SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(&app_role)
        .await
        .unwrap();
        assert_eq!(flags, (false, false));
        println!("W5 actual app role: superuser=false bypassrls=false");
        app_role.close().await;
        let (app, cookie, actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "WATCH", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"읽기와 연구"}),
        )
        .await;
        let task_id = task["id"].as_str().unwrap();
        let url = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}/timer");
        for table in [
            "task_timer_runs",
            "task_timer_segments",
            "task_timer_commands",
            "task_timer_audit",
            "task_timer_legacy_open",
        ] {
            assert_rls_forced(&admin, table).await;
        }
        let (status, started) = json_request(app.clone(), "POST", &url, Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"자료 읽기"})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let run = started["runId"].as_str().unwrap();
        let (status, paused) = json_request(
        app.clone(),
        "POST",
        &url,
        Some(
            json!({"requestId":Uuid::now_v7(),"operation":"pause","expectedVersion":1,"runId":run}),
        ),
        Some(&cookie),
    )
    .await;
        assert_eq!(status, StatusCode::OK, "{paused}");
        assert_eq!(paused["status"], "paused");
        let (status, fresh) = json_request(app.clone(), "GET", &url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        assert_eq!(fresh["run"]["id"], run);
        assert_eq!(fresh["run"]["version"], 2);
        assert_eq!(fresh["run"]["runningSince"], Value::Null);
        let elapsed: i64 = sqlx::query_scalar("SELECT sum(EXTRACT(EPOCH FROM (ended_at-started_at))*1000)::bigint FROM fvoci.task_timer_segments WHERE run_id=$1")
        .bind(Uuid::parse_str(run).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!(fresh["run"]["elapsedMilliseconds"], elapsed);
        let (status, resumed) = json_request(app.clone(), "POST", &url, Some(json!({"requestId":Uuid::now_v7(),"operation":"resume","expectedVersion":2,"runId":run})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{resumed}");
        let (status, stopped) = json_request(
        app.clone(),
        "POST",
        &url,
        Some(
            json!({"requestId":Uuid::now_v7(),"operation":"stop","expectedVersion":3,"runId":run}),
        ),
        Some(&cookie),
    )
    .await;
        assert_eq!(status, StatusCode::OK, "{stopped}");
        assert_eq!(stopped["status"], "stopped");
        let (status, fresh) = json_request(app.clone(), "GET", &url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        assert_eq!(fresh["run"], Value::Null);
        let (segments, closed, owner): (i64, i64, Uuid) = sqlx::query_as("SELECT count(*),count(ended_at),min(user_id::text)::uuid FROM fvoci.task_timer_segments WHERE run_id=$1")
        .bind(Uuid::parse_str(run).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!((segments, closed, owner), (2, 2, actor));
        let (status, after) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{workspace}/tasks/{task_id}"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            after["statusId"], task["statusId"],
            "stop must not complete task or trigger recurrence"
        );
        admin.close().await;
        drop(app);
        harness.cleanup().await;
    }
}
