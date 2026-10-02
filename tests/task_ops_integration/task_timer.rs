mod task_timer {
    // First timer consumer: real HTTP boundary, committed intervals, fresh read.
    use super::*;

    async fn captured(app: axum::Router, cookie: &str, mut body: Value) -> Value {
        let (status, me) = json_request(app, "GET", "/api/v1/auth/me", None, Some(cookie)).await;
        assert_eq!(status, StatusCode::OK, "{me}");
        body["expectedActorId"] = me["userId"].clone();
        body["expectedSessionId"] = me["sessionId"].clone();
        body
    }

    async fn timer_request(
        app: axum::Router,
        method: &str,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> (StatusCode, Value) {
        let body = match (body, cookie) {
            (Some(body), Some(cookie)) if method == "POST" && path.ends_with("/timer") => {
                Some(captured(app.clone(), cookie, body).await)
            }
            (body, _) => body,
        };
        json_request(app, method, path, body, cookie).await
    }

    #[tokio::test]
    async fn timer_capability_reuses_time_entry_policy_and_preserves_legacy_reads() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, _actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "CAP", "private").await;
        let viewer = add_workspace_user(&admin, workspace, "member", "timer-viewer").await;
        add_project_member(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            viewer.user_id,
            "viewer",
        )
        .await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Capability from existing policy"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}",
            task["id"].as_str().unwrap()
        );
        let (status, entry) = json_request(app.clone(), "POST", &format!("{base}/time-entries"), Some(json!({"startedAt":"2026-09-30T10:00:00Z","endedAt":"2026-09-30T10:01:00Z","note":"Unmodified shared legacy history"})), Some(&cookie)).await;
        assert!(status.is_success(), "{entry}");
        let member_url = format!(
            "/api/v1/workspaces/{workspace}/projects/{}/members/{}",
            project["id"].as_str().unwrap(),
            viewer.user_id
        );
        for (role, can_control) in [
            ("viewer", false),
            ("member", true),
            ("viewer", false),
            ("member", true),
        ] {
            let (status, changed) = json_request(
                app.clone(),
                "PATCH",
                &member_url,
                Some(json!({"role":role})),
                Some(&cookie),
            )
            .await;
            assert!(status.is_success(), "{changed}");
            let (status, state) = json_request(
                app.clone(),
                "GET",
                &format!("{base}/timer"),
                None,
                Some(&viewer.cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{state}");
            assert_eq!(state["canControl"], can_control);
            let (status, entries) = json_request(
                app.clone(),
                "GET",
                &format!("{base}/time-entries"),
                None,
                Some(&viewer.cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{entries}");
            assert_eq!(entries["canCreate"], can_control);
            assert_eq!(entries["items"].as_array().unwrap().len(), 1);
            assert_eq!(
                entries["items"][0]["note"],
                "Unmodified shared legacy history"
            );
            assert_eq!(entries["items"][0]["durationSeconds"], 60);
        }
        let archive_url = format!(
            "/api/v1/workspaces/{workspace}/projects/{}/archive",
            project["id"].as_str().unwrap()
        );
        let (status, archived) =
            json_request(app.clone(), "POST", &archive_url, None, Some(&cookie)).await;
        assert!(status.is_success(), "{archived}");
        let (status, state) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/timer"),
            None,
            Some(&viewer.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{state}");
        assert_eq!(state["canControl"], false);
        let (status, entries) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/time-entries"),
            None,
            Some(&viewer.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{entries}");
        assert_eq!(entries["canCreate"], false);
        assert_eq!(entries["items"][0]["durationSeconds"], 60);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_commands")
                .fetch_one(&admin)
                .await
                .unwrap(),
            0,
            "capability reads never create commands"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_runs")
                .fetch_one(&admin)
                .await
                .unwrap(),
            0,
            "capability reads never create runs"
        );
        admin.close().await;
        drop(app);
        harness.cleanup().await;
    }

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
        let (status, started) = timer_request(app.clone(), "POST", &url, Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"자료 읽기"})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let run = started["runId"].as_str().unwrap();
        let (status, paused) = timer_request(
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
        let (status, fresh) = timer_request(app.clone(), "GET", &url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        assert_eq!(fresh["run"]["id"], run);
        assert_eq!(fresh["run"]["version"], 2);
        assert_eq!(fresh["run"]["runningSince"], Value::Null);
        let elapsed: i64 = sqlx::query_scalar("SELECT sum(EXTRACT(EPOCH FROM (ended_at-started_at))*1000)::bigint FROM fvoci.task_timer_segments WHERE run_id=$1")
        .bind(Uuid::parse_str(run).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!(fresh["run"]["elapsedMilliseconds"], elapsed);
        let (status, resumed) = timer_request(app.clone(), "POST", &url, Some(json!({"requestId":Uuid::now_v7(),"operation":"resume","expectedVersion":2,"runId":run})), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{resumed}");
        let (status, stopped) = timer_request(
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
        let (status, fresh) = timer_request(app.clone(), "GET", &url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        assert_eq!(fresh["run"], Value::Null);
        let (segments, closed, owner): (i64, i64, Uuid) = sqlx::query_as("SELECT count(*),count(ended_at),min(user_id::text)::uuid FROM fvoci.task_timer_segments WHERE run_id=$1")
        .bind(Uuid::parse_str(run).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!((segments, closed, owner), (2, 2, actor));
        let (status, after) = timer_request(
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
    #[tokio::test]
    async fn timer_receipt_replay_and_actual_app_connection_rls_isolation() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let other = add_workspace_user(&admin, workspace, "member", "timer-other").await;
        let project = create_project(app.clone(), &cookie, workspace, "REPLAY", "workspace").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Replay once"}),
        )
        .await;
        let url = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        let body = json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"original"});
        let (status, original) =
            timer_request(app.clone(), "POST", &url, Some(body.clone()), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{original}");
        let (status, replayed) =
            timer_request(app.clone(), "POST", &url, Some(body.clone()), Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(
            original, replayed,
            "lost-success duplicate must return the committed receipt"
        );
        let mut changed = body.clone();
        changed["note"] = json!("changed");
        let (status, mismatch) =
            timer_request(app.clone(), "POST", &url, Some(changed), Some(&cookie)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{mismatch}");
        let pool = project_harness::app_pool(&harness).await;
        let mut tx = pool.begin().await.unwrap();
        let (superuser,bypass,nonowner,active): (bool,bool,bool,bool) = sqlx::query_as("SELECT r.rolsuper,r.rolbypassrls,c.relowner <> r.oid,row_security_active(c.oid) FROM pg_roles r CROSS JOIN pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE r.rolname=current_user AND n.nspname='fvoci' AND c.relname='task_timer_runs'").fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            (superuser, bypass, nonowner, active),
            (false, false, true, true)
        );
        fvoci_server::db::context::set_self_user(&mut tx, actor)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_runs")
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            1
        );
        fvoci_server::db::context::set_self_user(&mut tx, other.user_id)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_runs")
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_commands")
                .fetch_one(&mut *tx)
                .await
                .unwrap(),
            0
        );
        tx.commit().await.unwrap();
        let mut reused = pool.begin().await.unwrap();
        let self_id: Option<String> =
            sqlx::query_scalar("SELECT nullif(current_setting('app.self_user_id',true),'')")
                .fetch_one(&mut *reused)
                .await
                .unwrap();
        assert_eq!(
            self_id, None,
            "transaction-local actor must not leak on pool reuse"
        );
        reused.commit().await.unwrap();
        println!("W5 same app connection NSNB=false/false nonowner=true row_security_active=true actor-switch isolated");
        project_harness::close_pool(pool).await;
        admin.close().await;
        drop(app);
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn timer_two_independent_db_waiters_cross_workspace_start_one_person_run() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, actor, workspace_a) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let workspace_b = insert_workspace(&admin).await;
        sqlx::query(
            "INSERT INTO fvoci.memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        )
        .bind(workspace_b)
        .bind(actor)
        .execute(&admin)
        .await
        .unwrap();
        let pa = create_project(app.clone(), &cookie, workspace_a, "RUNONE", "workspace").await;
        let pb = create_project(app.clone(), &cookie, workspace_b, "RUNTWO", "workspace").await;
        let ta = create_task(
            app.clone(),
            &cookie,
            workspace_a,
            pa["id"].as_str().unwrap(),
            json!({"title":"A"}),
        )
        .await;
        let tb = create_task(
            app.clone(),
            &cookie,
            workspace_b,
            pb["id"].as_str().unwrap(),
            json!({"title":"B"}),
        )
        .await;
        let mut holder = admin.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        hold_membership_user_lock(&mut holder, actor).await;
        let a = app.clone();
        let ca = cookie.clone();
        let b = app.clone();
        let cb = cookie.clone();
        let ua = format!(
            "/api/v1/workspaces/{workspace_a}/tasks/{}/timer",
            ta["id"].as_str().unwrap()
        );
        let ub = format!(
            "/api/v1/workspaces/{workspace_b}/tasks/{}/timer",
            tb["id"].as_str().unwrap()
        );
        let first = tokio::spawn(async move {
            timer_request(a,"POST",&ua,Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null})),Some(&ca)).await
        });
        let second = tokio::spawn(async move {
            timer_request(b,"POST",&ub,Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null})),Some(&cb)).await
        });
        let waiters = project_harness::wait_for_blocked_query_count(
            &admin,
            pid,
            "%pg_advisory_xact_lock%",
            2,
        )
        .await;
        assert_ne!(
            waiters[0], waiters[1],
            "independent database connections must reach the actual writer barrier"
        );
        holder.commit().await.unwrap();
        let x = first.await.unwrap();
        let y = second.await.unwrap();
        assert!(
            (x.0 == StatusCode::OK && y.0 == StatusCode::CONFLICT)
                || (y.0 == StatusCode::OK && x.0 == StatusCode::CONFLICT),
            "{x:?} {y:?}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.task_timer_runs WHERE user_id=$1 AND status<>'stopped'"
            )
            .bind(actor)
            .fetch_one(&admin)
            .await
            .unwrap(),
            1
        );
        println!(
            "W5 actual barrier: two independent app DB waiters; cross-workspace one winner, one409"
        );
        admin.close().await;
        drop(app);
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn timer_command_without_captured_actor_session_cannot_execute_under_replaced_cookie() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, _actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let replacement = add_workspace_user(&admin, workspace, "member", "replacement").await;
        let project = create_project(app.clone(), &cookie, workspace, "SWAP", "workspace").await;
        add_project_member(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            replacement.user_id,
            "member",
        )
        .await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Old screen target"}),
        )
        .await;
        let url = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        // Exact original UI wire body captured before a cookie/actor switch.
        // Omitting context must be rejected, even if replacement has Edit.
        let (status,response)=json_request(app.clone(),"POST",&url,Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"old actor draft"})),Some(&replacement.cookie)).await;
        let effects: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fvoci.task_timer_runs WHERE user_id=$1")
                .bind(replacement.user_id)
                .fetch_one(&admin)
                .await
                .unwrap();
        println!("W5 original stale actor command status={status} replacement effects={effects} response={response}");
        admin.close().await;
        drop(app);
        harness.cleanup().await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "uncaptured old command must not execute under a replacement cookie"
        );
        assert_eq!(
            effects, 0,
            "no timer or old draft effect for replacement actor"
        );
    }
    #[tokio::test]
    async fn timer_captured_context_rejects_new_effects_but_fresh_session_replays_success() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let replacement = add_workspace_user(&admin, workspace, "member", "captured-other").await;
        let project = create_project(app.clone(), &cookie, workspace, "CONTEXT", "workspace").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Captured target"}),
        )
        .await;
        let url = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        let original=captured(app.clone(),&cookie,json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"actor-owned"})).await;
        let (status, wrong_actor) = json_request(
            app.clone(),
            "POST",
            &url,
            Some(original.clone()),
            Some(&replacement.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{wrong_actor}");
        assert_eq!(wrong_actor["params"]["code"], "timer_context_changed");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.task_timer_runs")
                .fetch_one(&admin)
                .await
                .unwrap(),
            0
        );
        let (status, success) = json_request(
            app.clone(),
            "POST",
            &url,
            Some(original.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{success}");
        let token = fvoci_server::auth::token::new_token();
        let fresh_session = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.sessions(id,user_id,token_hash,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '1 hour')").bind(fresh_session).bind(actor).bind(&token.hash).execute(&admin).await.unwrap();
        let (status, replayed) = json_request(
            app.clone(),
            "POST",
            &url,
            Some(original.clone()),
            Some(&token.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(
            replayed, success,
            "original committed receipt survives same-actor new session"
        );
        let mut refreshed_transport = original.clone();
        refreshed_transport["expectedSessionId"] = json!(fresh_session);
        let (status, replayed) = json_request(
            app.clone(),
            "POST",
            &url,
            Some(refreshed_transport),
            Some(&token.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(replayed, success);
        let mut stale_new = original.clone();
        stale_new["requestId"] = json!(Uuid::now_v7());
        let (status, stale) = json_request(
            app.clone(),
            "POST",
            &url,
            Some(stale_new),
            Some(&token.token),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale}");
        assert_eq!(stale["params"]["code"], "timer_context_changed");
        let pause=captured(app.clone(),&token.token,json!({"requestId":Uuid::now_v7(),"operation":"pause","expectedVersion":1,"runId":success["runId"]})).await;
        let (status, paused) =
            json_request(app.clone(), "POST", &url, Some(pause), Some(&token.token)).await;
        assert_eq!(status, StatusCode::OK, "{paused}");
        assert_eq!(paused["version"], 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id=$1"
            )
            .bind(actor)
            .fetch_one(&admin)
            .await
            .unwrap(),
            2,
            "one receipt per actual command; duplicate/guard failure creates none"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.task_timer_runs WHERE user_id=$1"
            )
            .bind(replacement.user_id)
            .fetch_one(&admin)
            .await
            .unwrap(),
            0
        );
        println!("W5 captured actor/session new effects rejected; same-actor fresh-session receipt preserved");
        admin.close().await;
        drop(app);
        harness.cleanup().await;
    }
    #[tokio::test]
    async fn timer_manual_midnight_correction_audit_history_and_existing_consumer_agree() {
        let harness = TestDb::bootstrap().await;
        let (app, cookie, actor, workspace) = setup_session(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "RECORD", "private").await;
        let other = add_workspace_user(&admin, workspace, "member", "record-other").await;
        add_project_member(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            other.user_id,
            "member",
        )
        .await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"기록과 보정"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        let original_start = "2026-09-30T14:59:30.100Z";
        let original_end = "2026-09-30T15:00:30.100Z";
        let other_body = captured(
            app.clone(),
            &other.cookie,
            json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T14:58:00Z","endedAt":"2026-09-30T14:59:00Z","note":"다른 작성자의 원본","reason":"다른 작성자의 사유"}),
        ).await;
        let (status, other_created) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/history"),
            Some(other_body),
            Some(&other.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{other_created}");
        let body=captured(app.clone(),&cookie,json!({"requestId":Uuid::now_v7(),"startedAt":original_start,"endedAt":original_end,"note":"원본 메모","reason":"수동 기록"})).await;
        let (status, created) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/history"),
            Some(body.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let id = created["record"]["id"].as_str().unwrap();
        let (status, replayed) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/history"),
            Some(body),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(replayed, created);
        let summary_url = format!("{base}/summary?from=2026-09-30&to=2026-10-01");
        let (status, total) =
            json_request(app.clone(), "GET", &summary_url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{total}");
        assert_eq!(total["totalMilliseconds"], 60_000);
        assert_eq!(total["days"][0]["milliseconds"], 29_900);
        assert_eq!(total["days"][1]["milliseconds"], 30_100);
        let correction=captured(app.clone(),&cookie,json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":0,"expectedStartedAt":original_start,"expectedEndedAt":original_end,"expectedNote":"원본 메모","startedAt":original_start,"endedAt":"2026-09-30T15:00:00.600Z","note":"휴식 제외","reason":"잘못 더한 휴식 29.5초 제외"})).await;
        let (status, corrected) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(correction.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{corrected}");
        assert_eq!(corrected["record"]["revision"], 1);
        let (status, replayed) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(correction.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{replayed}");
        assert_eq!(replayed, corrected);
        let mut stale = correction.clone();
        stale["requestId"] = json!(Uuid::now_v7());
        let (status, conflict) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(stale),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
        let (status, total) =
            json_request(app.clone(), "GET", &summary_url, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{total}");
        assert_eq!(total["totalMilliseconds"], 30_500);
        assert_eq!(total["days"][0]["milliseconds"], 29_900);
        assert_eq!(total["days"][1]["milliseconds"], 600);
        let (status, history) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/history?from=2026-09-30&to=2026-10-01"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{history}");
        assert_eq!(history["items"][0], corrected["record"]);
        let (raw_end, raw_note): (chrono::DateTime<chrono::Utc>, Option<String>) = sqlx::query_as(
            "SELECT ended_at,note FROM fvoci.time_entries WHERE id=$1 AND user_id=$2",
        )
        .bind(Uuid::parse_str(id).unwrap())
        .bind(actor)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            raw_end,
            chrono::DateTime::parse_from_rfc3339(original_end)
                .unwrap()
                .with_timezone(&chrono::Utc)
        );
        assert_eq!(
            raw_note.as_deref(),
            Some("원본 메모"),
            "original closed range/note stay preserved"
        );
        let (before,after,reason):(Value,Value,String)=sqlx::query_as("SELECT before_value,after_value,reason FROM fvoci.task_timer_audit WHERE user_id=$1 AND time_entry_id=$2 AND verb='time.correct'").bind(actor).bind(Uuid::parse_str(id).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!(before["endedAt"], original_end);
        assert_eq!(after["revision"], 1);
        assert_eq!(reason, "잘못 더한 휴식 29.5초 제외");
        let (status, existing) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{workspace}/tasks/{}/time-entries",
                task["id"].as_str().unwrap()
            ),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{existing}");
        assert_eq!(existing["items"][0]["durationSeconds"], 30);
        assert_eq!(existing["items"][1]["id"], other_created["record"]["id"]);
        assert_eq!(existing["items"][1]["note"], "다른 작성자의 원본");
        assert_eq!(existing["items"][1]["durationSeconds"], 60);
        let effective_end = existing["items"][0]["endedAt"].clone();
        let effective_note = existing["items"][0]["note"].clone();
        let existing_url = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/time-entries",
            task["id"].as_str().unwrap()
        );
        let (status, shared) =
            json_request(app.clone(), "GET", &existing_url, None, Some(&other.cookie)).await;
        assert_eq!(status, StatusCode::OK, "{shared}");
        assert_eq!(shared["items"][0]["endedAt"], original_end);
        assert_eq!(shared["items"][0]["note"], "원본 메모");
        assert_eq!(shared["items"][0]["durationSeconds"], 60);
        assert!(!shared.to_string().contains("휴식 제외"));
        assert!(!shared.to_string().contains("29.5초"));
        let (status, other_history) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/history?from=2026-09-30&to=2026-10-01"),
            None,
            Some(&other.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{other_history}");
        assert_eq!(other_history["items"].as_array().unwrap().len(), 1);
        assert_eq!(other_history["items"][0], other_created["record"]);
        let effects_before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.task_timer_commands),(SELECT count(*) FROM fvoci.task_timer_audit),(SELECT count(*) FROM fvoci.time_entries)").fetch_one(&admin).await.unwrap();
        let wrong_owner = captured(app.clone(), &other.cookie, correction).await;
        let (status, denied) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(wrong_owner),
            Some(&other.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
        let (status, _) = json_request(app.clone(), "GET", &existing_url, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        sqlx::query("DELETE FROM fvoci.project_members WHERE project_id=$1 AND user_id=$2")
            .bind(Uuid::parse_str(project["id"].as_str().unwrap()).unwrap())
            .bind(other.user_id)
            .execute(&admin)
            .await
            .unwrap();
        let (status, denied) =
            json_request(app.clone(), "GET", &existing_url, None, Some(&other.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
        assert!(!denied.to_string().contains("원본 메모"));
        let effects_after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.task_timer_commands),(SELECT count(*) FROM fvoci.task_timer_audit),(SELECT count(*) FROM fvoci.time_entries)").fetch_one(&admin).await.unwrap();
        assert_eq!(
            effects_after, effects_before,
            "other-owner correction and denied reads have no history/audit/receipt effects"
        );
        // The actual production pool factory restricted to one connection
        // proves reuse rather than assuming two checkouts chose the same PID.
        let witness = fvoci_server::db::pool::connect_app_with_max(&harness.app_url, 1)
            .await
            .unwrap();
        let pid_before: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&witness)
            .await
            .unwrap();
        let flags: Vec<(String, bool, bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT c.relname,r.rolsuper,r.rolbypassrls,c.relowner<>r.oid,c.relforcerowsecurity,row_security_active(c.oid) FROM pg_roles r CROSS JOIN pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE r.rolname=current_user AND n.nspname='fvoci' AND c.relname=ANY($1) ORDER BY c.relname",
        )
        .bind(vec!["time_entries", "task_timer_runs", "task_timer_segments", "task_timer_legacy_open", "task_timer_commands", "task_timer_audit"])
        .fetch_all(&witness)
        .await
        .unwrap();
        assert_eq!(flags.len(), 6);
        for (name, superuser, bypass, nonowner, force, active) in &flags {
            assert_eq!(
                (*superuser, *bypass, *nonowner, *force, *active),
                (false, false, true, true, true),
                "{name}"
            );
        }
        let session = project_harness::session_id_for_user(&admin, actor).await;
        let own_read = fvoci_server::db::task_ops::list_time_entries(
            &witness,
            workspace,
            Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
            actor,
            session,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(own_read.items[0].note.as_deref(), Some("휴식 제외"));
        let context_sql = "SELECT pg_backend_pid(),NULLIF(current_setting('app.self_user_id',true),''),NULLIF(current_setting('app.tenant_id',true),''),NULLIF(current_setting('app.system_ctx',true),'')";
        let after_read: (i32, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(context_sql)
                .fetch_one(&witness)
                .await
                .unwrap();
        assert_eq!(after_read, (pid_before, None, None, None));
        let mut reused = fvoci_server::db::context::begin_read(&witness)
            .await
            .unwrap();
        fvoci_server::db::context::set_tenant(&mut reused, workspace)
            .await
            .unwrap();
        fvoci_server::db::context::set_self_user(&mut reused, other.user_id)
            .await
            .unwrap();
        let during: (i32, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(context_sql)
                .fetch_one(&mut *reused)
                .await
                .unwrap();
        assert_eq!(
            during,
            (
                pid_before,
                Some(other.user_id.to_string()),
                Some(workspace.to_string()),
                None
            )
        );
        let own_audits: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_timer_audit")
            .fetch_one(&mut *reused)
            .await
            .unwrap();
        assert_eq!(
            own_audits, 1,
            "same backend switched actor sees only their manual audit"
        );
        reused.commit().await.unwrap();
        let after_reuse: (i32, Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(context_sql)
                .fetch_one(&witness)
                .await
                .unwrap();
        assert_eq!(after_reuse, (pid_before, None, None, None));
        println!("W5 current read witness six NSNB=false/false nonowner/FORCE/RLSactive=true; same backend PID={pid_before}; local self/tenant/system after read/reuse empty");
        project_harness::close_pool(witness).await;
        println!("W5 manual/correction canonical/day/history audit phases passed; original range preserved; existing consumer endedAt={effective_end} note={effective_note}");
        admin.close().await;
        drop(app);
        harness.cleanup().await;
        assert_eq!(
            effective_end, corrected["record"]["endedAt"],
            "existing TaskTimeEntries consumer must show this actor's same effective correction"
        );
        assert_eq!(effective_note, corrected["record"]["note"]);
    }

    // Keep the actual production Router's sole app connection, not a separate
    // witness pool. Isolated TMPDIR is supplied by the coordinator's batch.
    struct TimerFixture {
        pool: sqlx::PgPool,
        pid: i32,
        password_keys: fvoci_server::auth::password::Keyring,
        storage_root: std::path::PathBuf,
    }

    impl Drop for TimerFixture {
        fn drop(&mut self) {
            // This exact root was created and captured by this fixture.
            if self.storage_root.exists() {
                std::fs::remove_dir_all(&self.storage_root).expect("remove owned timer storage");
            }
        }
    }

    impl TimerFixture {
        async fn setup(harness: &TestDb) -> (Self, axum::Router, String, Uuid, Uuid) {
            let mut state = project_harness::app_state(&harness.app_url).await;
            let storage_root = match &state.storage {
                fvoci_server::attachments::ObjectStorage::Local(storage) => {
                    storage.root().to_path_buf()
                }
                _ => panic!("timer fixture requires its own local storage"),
            };
            assert!(storage_root.starts_with(std::env::temp_dir()));
            println!("W5 owned storage before body: {}", storage_root.display());
            let pool = fvoci_server::db::pool::connect_app_with_max(&harness.app_url, 1)
                .await
                .unwrap();
            let old_pool = state.auth.db.pool.clone();
            let password_keys = state.auth.password_keys.clone();
            state.auth = std::sync::Arc::new(fvoci_server::auth::AuthService {
                db: fvoci_server::db::Db::new(pool.clone()),
                password_keys: password_keys.clone(),
            });
            project_harness::close_pool(old_pool).await;
            let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&pool)
                .await
                .unwrap();
            let fixture = Self {
                pool,
                pid,
                password_keys,
                storage_root,
            };
            fixture.probe("before setup").await;
            let app = fvoci_server::http::router(state, None);
            let (status, response, headers) = project_harness::json_request_with_headers(
                app.clone(),
                "POST",
                "/api/v1/setup",
                Some(json!({
                    "email":"owner@example.com", "password":"supersecret1", "givenName":"Owner",
                    "workspaceSlug":"acme", "workspaceName":"Acme"
                })),
                None,
            )
            .await;
            fixture.probe("after setup").await;
            assert!(status.is_success(), "{response}");
            let cookie = timer_session_cookie(&headers);
            let (status, me) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                "/api/v1/auth/me",
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{me}");
            let actor = Uuid::parse_str(me["userId"].as_str().unwrap()).unwrap();
            let (status, workspaces) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                "/api/v1/me/workspaces",
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{workspaces}");
            let workspace = workspaces["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["slug"] == "acme")
                .unwrap();
            let workspace = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
            (fixture, app, cookie, actor, workspace)
        }

        async fn login(&self, app: axum::Router, email: &str, password: &str) -> (String, Value) {
            self.probe("before actual login").await;
            let (status, response, headers) = project_harness::json_request_with_headers(
                app.clone(),
                "POST",
                "/api/v1/auth/login",
                Some(json!({"email":email,"password":password})),
                None,
            )
            .await;
            self.probe("after actual login").await;
            assert_eq!(status, StatusCode::OK, "{response}");
            let cookie = timer_session_cookie(&headers);
            let (status, me) =
                timer_checked_request(self, app, "GET", "/api/v1/auth/me", None, Some(&cookie))
                    .await;
            assert_eq!(status, StatusCode::OK, "{me}");
            (cookie, me)
        }

        async fn probe(&self, phase: &str) {
            use sqlx::Row;
            let rows = sqlx::query(
                r#"SELECT pg_backend_pid() AS pid, current_user::text AS role,
                r.rolsuper AS superuser, r.rolbypassrls AS bypass,
                c.relname::text AS table_name, c.relowner <> r.oid AS nonowner,
                c.relrowsecurity AS enabled, c.relforcerowsecurity AS forced,
                row_security_active(c.oid) AS active,
                nullif(current_setting('app.self_user_id',true),'') AS self_id,
                nullif(current_setting('app.tenant_id',true),'') AS tenant_id,
                nullif(current_setting('app.system_ctx',true),'') AS system_ctx
                FROM pg_roles r CROSS JOIN pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
                WHERE r.rolname=current_user AND n.nspname='fvoci'
                AND c.relname IN ('task_timer_runs','task_timer_segments','task_timer_commands',
                  'task_timer_audit','task_timer_legacy_open','time_entries') ORDER BY c.relname"#,
            )
            .fetch_all(&self.pool)
            .await
            .unwrap();
            assert_eq!(rows.len(), 6, "{phase}");
            for row in rows {
                assert_eq!(
                    row.get::<i32, _>("pid"),
                    self.pid,
                    "same actual Router backend: {phase}"
                );
                assert!(
                    !row.get::<bool, _>("superuser") && !row.get::<bool, _>("bypass"),
                    "{phase}"
                );
                for flag in ["nonowner", "enabled", "forced", "active"] {
                    assert!(
                        row.get::<bool, _>(flag),
                        "{} {flag}: {phase}",
                        row.get::<String, _>("table_name")
                    );
                }
                for setting in ["self_id", "tenant_id", "system_ctx"] {
                    assert_eq!(
                        row.get::<Option<String>, _>(setting),
                        None,
                        "{setting}: {phase}"
                    );
                }
            }
        }

        async fn close(self) {
            self.probe("final reuse").await;
            project_harness::close_pool(self.pool.clone()).await;
            println!("W5 actual Router pool: pid={} six NSNB/nonowner/FORCE/RLS-active tables; locals empty", self.pid);
            std::fs::remove_dir_all(&self.storage_root).unwrap();
            assert!(!self.storage_root.exists());
        }
    }

    fn timer_session_cookie(headers: &axum::http::HeaderMap) -> String {
        headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| {
                axum_extra::extract::cookie::Cookie::parse(value.to_str().ok()?.to_owned()).ok()
            })
            .find(|cookie| cookie.name() == "fvoci_session")
            .unwrap()
            .value()
            .to_owned()
    }

    async fn timer_checked_request(
        fixture: &TimerFixture,
        app: axum::Router,
        method: &str,
        path: &str,
        body: Option<Value>,
        cookie: Option<&str>,
    ) -> (StatusCode, Value) {
        fixture.probe("before HTTP").await;
        let response = json_request(app, method, path, body, cookie).await;
        fixture.probe("after HTTP").await;
        response
    }

    async fn timer_effects(admin: &sqlx::PgPool, actors: &[Uuid], tasks: &[Uuid]) -> Value {
        sqlx::query_scalar(r#"SELECT jsonb_build_object(
      'runs',(SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]'::jsonb) FROM fvoci.task_timer_runs r WHERE r.user_id=ANY($1)),
      'segments',(SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY s.id),'[]'::jsonb) FROM fvoci.task_timer_segments s WHERE s.user_id=ANY($1)),
      'receipts',(SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY c.user_id,c.request_id),'[]'::jsonb) FROM fvoci.task_timer_commands c WHERE c.user_id=ANY($1)),
      'audit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.task_timer_audit a WHERE a.user_id=ANY($1)),
      'legacy',(SELECT COALESCE(jsonb_agg(to_jsonb(l) ORDER BY l.time_entry_id),'[]'::jsonb) FROM fvoci.task_timer_legacy_open l WHERE l.user_id=ANY($1)),
      'history',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.time_entries e WHERE e.user_id=ANY($1)),
      'events',(SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]'::jsonb) FROM fvoci.events e WHERE e.target_id=ANY($2)),
      'taskAudit',(SELECT COALESCE(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]'::jsonb) FROM fvoci.audit_log a WHERE a.target_id=ANY($2)),
      'tasks',(SELECT COALESCE(jsonb_agg(jsonb_build_object('id',t.id,'statusId',t.status_id,'startDate',t.start_date,'dueDate',t.due_date,'dueAt',t.due_at,'recurrence',t.recurrence,'estimate',t.estimate,'updatedAt',t.updated_at) ORDER BY t.id),'[]'::jsonb) FROM fvoci.tasks t WHERE t.id=ANY($2))
    )"#).bind(actors).bind(tasks).fetch_one(admin).await.unwrap()
    }

    #[tokio::test]
    async fn timer_manual_nanosecond_echo_history_and_correction_roundtrip() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, _actor, workspace) = TimerFixture::setup(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "PRECISION", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Precision roundtrip"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T14:59:30.100000123Z","endedAt":"2026-09-30T14:59:31.100000456Z","note":"Original precision","reason":"Explicit manual range"})).await;
        let (create_status, created) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{base}/history"),
            Some(body),
            Some(&cookie),
        )
        .await;
        assert_eq!(create_status, StatusCode::OK, "{created}");
        let id = created["record"]["id"].as_str().unwrap();
        let (history_status, history) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/history?from=2026-09-30&to=2026-09-30"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(history_status, StatusCode::OK, "{history}");
        let correction = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":0,"expectedStartedAt":created["record"]["startedAt"],"expectedEndedAt":created["record"]["endedAt"],"expectedNote":created["record"]["note"],"startedAt":created["record"]["startedAt"],"endedAt":created["record"]["endedAt"],"note":"Corrected note","reason":"Echoed committed baseline"})).await;
        let (correction_status, corrected) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(correction),
            Some(&cookie),
        )
        .await;
        println!(
        "W5 precision original: create={create_status}, history={history_status}, correction={correction_status}, roundtripEqual={}",
        history["items"][0] == created["record"]
    );
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
        assert_eq!(
            history["items"][0], created["record"],
            "committed response must equal fresh history"
        );
        assert_eq!(
            correction_status,
            StatusCode::OK,
            "echoed committed baseline cannot be immediately stale: {corrected}"
        );
        assert_eq!(
            created["record"]["startedAt"], "2026-09-30T14:59:30.100Z",
            "new ranges explicitly normalize to milliseconds"
        );
        assert_eq!(created["record"]["endedAt"], "2026-09-30T14:59:31.100Z");
    }

    #[tokio::test]
    async fn timer_manual_submillisecond_second_boundary_agrees_with_existing_consumer() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, _actor, workspace) = TimerFixture::setup(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "SECONDS", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Endpoint millisecond boundary"}),
        )
        .await;
        let task_url = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}",
            task["id"].as_str().unwrap()
        );
        let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T14:00:00.100999Z","endedAt":"2026-09-30T14:00:02.100000Z","note":null,"reason":"Whole-second boundary"})).await;
        let (status, created) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{task_url}/timer/history"),
            Some(body),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let (status, state) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{task_url}/timer"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{state}");
        let (status, entries) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{task_url}/time-entries"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{entries}");
        let actual = state["actualMilliseconds"].as_i64().unwrap();
        let seconds = entries["items"][0]["durationSeconds"].as_i64().unwrap();
        println!(
            "W5 endpoint rounding original: actualMilliseconds={actual}, existingSeconds={seconds}"
        );
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
        assert_eq!(actual, 2000);
        assert_eq!(
            seconds,
            actual / 1000,
            "same effective milliseconds must determine existing whole-second projection"
        );
    }

    #[tokio::test]
    async fn timer_history_exact_local_midnight_is_half_open_in_seoul_and_dst_days() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "DAYEDGE", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Local calendar boundaries"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        let mut results = Vec::new();
        for (zone, from, next, start, end, elapsed) in [
            (
                "Asia/Seoul",
                "2026-09-30",
                "2026-10-01",
                "2026-09-30T14:59:30Z",
                "2026-09-30T15:00:00Z",
                30000_i64,
            ),
            (
                "America/New_York",
                "2026-03-08",
                "2026-03-09",
                "2026-03-08T05:00:00Z",
                "2026-03-09T04:00:00Z",
                82800000,
            ),
            (
                "America/New_York",
                "2025-11-02",
                "2025-11-03",
                "2025-11-02T04:00:00Z",
                "2025-11-03T05:00:00Z",
                90000000,
            ),
        ] {
            // Controlled fixture preference only; actual read converts through PG timezone.
            sqlx::query("UPDATE fvoci.users SET timezone=$2 WHERE id=$1")
                .bind(actor)
                .bind(zone)
                .execute(&admin)
                .await
                .unwrap();
            let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":start,"endedAt":end,"note":null,"reason":"Exact local day end"})).await;
            let (status, value) = timer_checked_request(
                &fixture,
                app.clone(),
                "POST",
                &format!("{base}/history"),
                Some(body),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{value}");
            let (status, summary) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{base}/summary?from={from}&to={next}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{summary}");
            let (status, current_history) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{base}/history?from={from}&to={from}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{current_history}");
            let (status, next_history) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{base}/history?from={next}&to={next}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{next_history}");
            results.push((zone, elapsed, summary, current_history, next_history));
        }
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
        for (zone, elapsed, summary, current, next) in results {
            assert_eq!(summary["timeZone"], zone);
            assert_eq!(summary["totalMilliseconds"], elapsed);
            assert_eq!(summary["days"][0]["milliseconds"], elapsed);
            assert_eq!(summary["days"][1]["milliseconds"], 0);
            assert_eq!(current["items"].as_array().unwrap().len(), 1);
            assert!(
                next["items"].as_array().unwrap().is_empty(),
                "half-open end belongs solely to the previous local day: {zone}"
            );
        }
    }

    #[tokio::test]
    async fn timer_history_equal_anchor_over_one_page_rejects_correction_between_pages() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, _actor, workspace) = TimerFixture::setup(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "CURSOR", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Correction during pagination"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        for _ in 0..101 {
            let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T10:00:00Z","endedAt":"2026-09-30T10:00:30Z","note":null,"reason":"Independent equal-anchor history fixture"})).await;
            let (status, value) = timer_checked_request(
                &fixture,
                app.clone(),
                "POST",
                &format!("{base}/history"),
                Some(body),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        let path = format!("{base}/history?from=2026-09-30&to=2026-09-30");
        let (status, first) =
            timer_checked_request(&fixture, app.clone(), "GET", &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["items"].as_array().unwrap().len(), 100);
        let selected = first["items"][0].clone();
        let id = selected["id"].as_str().unwrap();
        let correction = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":0,"expectedStartedAt":selected["startedAt"],"expectedEndedAt":selected["endedAt"],"expectedNote":selected["note"],"startedAt":"2026-09-30T09:00:00Z","endedAt":"2026-09-30T09:00:30Z","note":null,"reason":"Move already-shown record across saved cursor"})).await;
        let (status, value) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{base}/records/{id}/correct"),
            Some(correction),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let cursor = first["nextCursor"].as_str().unwrap();
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("cursor", cursor)
            .finish();
        let (continuation_status, continuation) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{path}&{query}"),
            None,
            Some(&cookie),
        )
        .await;
        let duplicate = continuation["items"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == selected["id"]));
        println!(
        "W5 cursor original: firstCount=100, continuationStatus={continuation_status}, duplicateShownRecord={duplicate}"
    );
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
        assert_eq!(
            continuation_status,
            StatusCode::CONFLICT,
            "changed personal history requires explicit stale continuation/refetch"
        );
        assert_eq!(continuation["params"]["code"], "timer_history_changed");
    }

    async fn timer_no_effect_request(
        fixture: &TimerFixture,
        admin: &sqlx::PgPool,
        actors: &[Uuid],
        tasks: &[Uuid],
        app: axum::Router,
        path: &str,
        body: Value,
        cookie: &str,
        expected_status: StatusCode,
    ) -> Value {
        let before = timer_effects(admin, actors, tasks).await;
        let (status, response) =
            timer_checked_request(fixture, app, "POST", path, Some(body), Some(cookie)).await;
        let after = timer_effects(admin, actors, tasks).await;
        assert_eq!(status, expected_status, "{response}");
        assert_eq!(
            after, before,
            "all ordered timer/history/task effects must remain unchanged"
        );
        response
    }

    fn assert_self_cleanup_effects(
        before: &Value,
        after: &Value,
        run: &Value,
        request: &Value,
        output: &Value,
    ) {
        for field in ["legacy", "history", "events", "taskAudit", "tasks"] {
            assert_eq!(
                after[field], before[field],
                "self cleanup cannot mutate {field}"
            );
        }
        for field in ["receipts", "audit"] {
            let old = before[field].as_array().unwrap();
            let current = after[field].as_array().unwrap();
            assert_eq!(current.len(), old.len() + 1, "one committed {field}");
            for row in old {
                assert!(
                    current.contains(row),
                    "old full {field} row must remain immutable"
                );
            }
            let added = current.iter().find(|row| !old.contains(row)).unwrap();
            assert_eq!(added["request_id"], *request);
            if field == "receipts" {
                assert_eq!(added["result"], *output);
                assert_eq!(added["run_id"], *run);
            } else {
                assert_eq!(added["verb"], "cleanup");
                assert!(
                    added["workspace_id"].is_null()
                        && added["task_id"].is_null()
                        && added["time_entry_id"].is_null()
                );
                assert_eq!(added["after_value"]["runId"], *run);
            }
        }
        let mut changed_run = 0;
        assert_eq!(
            after["runs"].as_array().unwrap().len(),
            before["runs"].as_array().unwrap().len()
        );
        for old in before["runs"].as_array().unwrap() {
            let current = after["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == old["id"])
                .unwrap();
            if old["id"] != *run {
                assert_eq!(old, current);
                continue;
            }
            changed_run += 1;
            let mut expected = old.clone();
            expected["status"] = json!("stopped");
            expected["version"] = output["version"].clone();
            assert!(current["stopped_at"].is_string());
            expected["stopped_at"] = current["stopped_at"].clone();
            assert_eq!(
                &expected, current,
                "only own run status/version/stop anchor change"
            );
        }
        assert_eq!(changed_run, 1);
        assert_eq!(
            after["segments"].as_array().unwrap().len(),
            before["segments"].as_array().unwrap().len()
        );
        let mut closed = 0;
        for old in before["segments"].as_array().unwrap() {
            let current = after["segments"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == old["id"])
                .unwrap();
            if old["run_id"] == *run && old["ended_at"].is_null() {
                assert!(current["ended_at"].is_string());
                assert!(current["time_entry_id"].is_null());
                let mut expected = old.clone();
                expected["ended_at"] = current["ended_at"].clone();
                assert_eq!(&expected, current);
                closed += 1;
            } else {
                assert_eq!(old, current, "other/closed full segment remains immutable");
            }
        }
        assert_eq!(closed, 1);
    }

    #[tokio::test]
    async fn timer_cleanup_revoked_receipt_replay_full_effects_and_actual_router_pool() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, owner_cookie, owner, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let actor = add_workspace_user(&admin, workspace, "member", "cleanup-owner").await;
        // Approved isolated account preparation only. Both relevant cookies
        // below are issued by the public production login path, never INSERTed.
        let email: String = sqlx::query_scalar("SELECT email FROM fvoci.users WHERE id=$1")
            .bind(actor.user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
        let password = "timer-fixture-password1";
        let password_hash =
            fvoci_server::auth::password::hash_password(password, &fixture.password_keys)
                .await
                .unwrap();
        sqlx::query("UPDATE fvoci.users SET password_hash=$2 WHERE id=$1")
            .bind(actor.user_id)
            .bind(password_hash)
            .execute(&admin)
            .await
            .unwrap();
        let (first_cookie, first_me) = fixture.login(app.clone(), &email, password).await;
        assert_eq!(first_me["userId"], json!(actor.user_id));
        let actor = project_harness::TestUser {
            user_id: actor.user_id,
            cookie: first_cookie,
        };
        let other = add_workspace_user(&admin, workspace, "member", "cleanup-other").await;
        let private =
            create_project(app.clone(), &owner_cookie, workspace, "HIDDEN", "private").await;
        add_project_member(
            app.clone(),
            &owner_cookie,
            workspace,
            private["id"].as_str().unwrap(),
            actor.user_id,
            "member",
        )
        .await;
        add_project_member(
            app.clone(),
            &owner_cookie,
            workspace,
            private["id"].as_str().unwrap(),
            other.user_id,
            "member",
        )
        .await;
        let task = create_task(app.clone(), &owner_cookie, workspace, private["id"].as_str().unwrap(),
            json!({"title":"Revoked private timer target","startDate":"2026-10-01","dueDate":"2026-10-30"})).await;
        // A second ordinary workspace is a real global-owner consumer.
        let next_workspace = insert_workspace(&admin).await;
        for user in [owner, actor.user_id, other.user_id] {
            sqlx::query(
                "INSERT INTO fvoci.memberships(workspace_id,user_id,role) VALUES($1,$2,$3)",
            )
            .bind(next_workspace)
            .bind(user)
            .bind(if user == owner { "owner" } else { "member" })
            .execute(&admin)
            .await
            .unwrap();
        }
        let project2 = create_project(
            app.clone(),
            &owner_cookie,
            next_workspace,
            "NEXT",
            "workspace",
        )
        .await;
        let task2 = create_task(
            app.clone(),
            &owner_cookie,
            next_workspace,
            project2["id"].as_str().unwrap(),
            json!({"title":"New visible target"}),
        )
        .await;
        let tasks = [
            Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
            Uuid::parse_str(task2["id"].as_str().unwrap()).unwrap(),
        ];
        let actors = [owner, actor.user_id, other.user_id];
        let url = format!("/api/v1/workspaces/{workspace}/tasks/{}/timer", tasks[0]);
        let url2 = format!(
            "/api/v1/workspaces/{next_workspace}/tasks/{}/timer",
            tasks[1]
        );
        let cleanup_url = "/api/v1/me/task-timer/stop";
        let start = captured(app.clone(), &actor.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"Private run note"})).await;
        let mut missing = start.clone();
        missing.as_object_mut().unwrap().remove("expectedActorId");
        missing.as_object_mut().unwrap().remove("expectedSessionId");
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &url,
            missing,
            &actor.cookie,
            StatusCode::BAD_REQUEST,
        )
        .await;
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &url,
            start.clone(),
            &other.cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let (status, started) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &url,
            Some(start.clone()),
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let run = started["runId"].clone();
        let pause = captured(
            app.clone(),
            &actor.cookie,
            json!({"requestId":Uuid::now_v7(),"operation":"pause","runId":run,"expectedVersion":1}),
        )
        .await;
        let (status, paused) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &url,
            Some(pause),
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{paused}");
        assert_eq!(paused["status"], "paused");
        let next_start = captured(app.clone(), &actor.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null})).await;
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &url2,
            next_start,
            &actor.cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let resume = captured(app.clone(), &actor.cookie, json!({"requestId":Uuid::now_v7(),"operation":"resume","runId":run,"expectedVersion":2})).await;
        let (status, resumed) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &url,
            Some(resume),
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{resumed}");
        assert_eq!(resumed["version"], 3);
        let cleanup = captured(
            app.clone(),
            &actor.cookie,
            json!({"requestId":Uuid::now_v7(),"runId":run,"expectedVersion":3}),
        )
        .await;
        let mut missing = cleanup.clone();
        missing.as_object_mut().unwrap().remove("expectedSessionId");
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            missing,
            &actor.cookie,
            StatusCode::BAD_REQUEST,
        )
        .await;
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            cleanup.clone(),
            &other.cookie,
            StatusCode::CONFLICT,
        )
        .await;
        // New same-actor credential is genuinely issued by login. Snapshot
        // timer denial effects only AFTER expected auth.login preparation.
        let (fresh_cookie, fresh_me) = fixture.login(app.clone(), &email, password).await;
        assert_eq!(fresh_me["userId"], first_me["userId"]);
        assert_ne!(fresh_me["sessionId"], first_me["sessionId"]);
        assert_eq!(start["expectedSessionId"], first_me["sessionId"]);
        let fresh_session = Uuid::parse_str(fresh_me["sessionId"].as_str().unwrap()).unwrap();
        assert_ne!(
            fresh_session,
            Uuid::parse_str(first_me["sessionId"].as_str().unwrap()).unwrap()
        );
        let mut stale_start = start.clone();
        stale_start["requestId"] = json!(Uuid::now_v7());
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &url,
            stale_start,
            &fresh_cookie,
            StatusCode::CONFLICT,
        )
        .await;
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            cleanup.clone(),
            &fresh_cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let mut changed = start.clone();
        changed["note"] = json!("Changed original payload");
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &url,
            changed,
            &actor.cookie,
            StatusCode::CONFLICT,
        )
        .await;

        // Current permission must precede successful receipt disclosure. Observe
        // the actual app backend waiting on an independent membership lock.
        let before_revoke = timer_effects(&admin, &actors, &tasks).await;
        let mut holder = admin.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        hold_membership_user_lock(&mut holder, actor.user_id).await;
        let replay_app = app.clone();
        let replay_cookie = actor.cookie.clone();
        let replay_url = url.clone();
        let replay_body = start.clone();
        let pending = tokio::spawn(async move {
            json_request(
                replay_app,
                "POST",
                &replay_url,
                Some(replay_body),
                Some(&replay_cookie),
            )
            .await
        });
        let waiter = wait_for_advisory_blocked_by(&admin, blocker).await;
        assert_ne!(waiter, blocker);
        assert_eq!(
            waiter, fixture.pid,
            "actual production Router backend at writer barrier"
        );
        sqlx::query("DELETE FROM fvoci.project_members WHERE project_id=$1 AND user_id=$2")
            .bind(Uuid::parse_str(private["id"].as_str().unwrap()).unwrap())
            .bind(actor.user_id)
            .execute(&mut *holder)
            .await
            .unwrap();
        holder.commit().await.unwrap();
        let (status, revoked) = pending.await.unwrap();
        fixture.probe("after ACL barrier").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{revoked}");
        assert!(
            revoked.get("runId").is_none(),
            "no committed private receipt disclosure"
        );
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before_revoke);
        let before_reads = timer_effects(&admin, &actors, &tasks).await;
        let (status, hidden) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &url,
            None,
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
        let (status, owner_state) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            "/api/v1/me/task-timer",
            None,
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{owner_state}");
        assert_eq!(owner_state["runId"], run);
        assert_eq!(owner_state["version"], 3);
        assert!(owner_state["visibleRun"].is_null());
        for private_field in ["workspaceId", "taskId", "title", "note"] {
            assert!(owner_state.get(private_field).is_none());
        }
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before_reads);

        let before_cleanup = timer_effects(&admin, &actors, &tasks).await;
        let (status, cleaned) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            cleanup_url,
            Some(cleanup.clone()),
            Some(&actor.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cleaned}");
        assert_eq!(cleaned["runId"], run);
        assert_eq!(cleaned["status"], "stopped");
        assert_eq!(cleaned["version"], 4);
        let after_cleanup = timer_effects(&admin, &actors, &tasks).await;
        assert_self_cleanup_effects(
            &before_cleanup,
            &after_cleanup,
            &run,
            &cleanup["requestId"],
            &cleaned,
        );
        // Exact successful cleanup receipt survives fresh session, with no new
        // audit, history, task event, or run/segment mutation.
        let replayed = timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            cleanup.clone(),
            &fresh_cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replayed, cleaned);
        let mut changed = cleanup.clone();
        changed["expectedVersion"] = json!(2);
        timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            changed,
            &fresh_cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let start2 = captured(app.clone(), &fresh_cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","runId":null,"expectedVersion":0,"note":"New target draft"})).await;
        let (status, run2) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &url2,
            Some(start2),
            Some(&fresh_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{run2}");
        assert_ne!(run2["runId"], run);
        let replayed = timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            cleanup,
            &fresh_cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replayed, cleaned);
        let (status, current) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            "/api/v1/me/task-timer",
            None,
            Some(&fresh_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{current}");
        assert_eq!(current["runId"], run2["runId"]);
        assert_eq!(current["status"], "running");
        let cleanup2 = captured(
            app.clone(),
            &fresh_cookie,
            json!({"requestId":Uuid::now_v7(),"runId":run2["runId"],"expectedVersion":1}),
        )
        .await;
        let before_final = timer_effects(&admin, &actors, &tasks).await;
        let (status, stopped2) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            cleanup_url,
            Some(cleanup2.clone()),
            Some(&fresh_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{stopped2}");
        assert_eq!(stopped2["runId"], run2["runId"]);
        assert_eq!(stopped2["status"], "stopped");
        let final_effects = timer_effects(&admin, &actors, &tasks).await;
        assert_self_cleanup_effects(
            &before_final,
            &final_effects,
            &run2["runId"],
            &cleanup2["requestId"],
            &stopped2,
        );
        let replayed = timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            cleanup_url,
            cleanup2,
            &fresh_cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replayed, stopped2);
        println!("W5 F3: full ordered-effect guards/revoked-before-receipt/opaque cleanup/fresh-session replay/R1 replay retains cross-workspace R2");
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn timer_manual_precision_raw_payload_audit_and_legacy_cas_are_preserved() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "MSAUDIT", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Submitted and effective ranges"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}");
        let submitted = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T10:00:00.100000123Z","endedAt":"2026-09-30T10:00:02.100000456Z","note":"new manual","reason":"Explicit submitted precision"})).await;
        let (status, created) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{base}/timer/history"),
            Some(submitted.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        assert_eq!(created["record"]["startedAt"], "2026-09-30T10:00:00.100Z");
        assert_eq!(created["record"]["endedAt"], "2026-09-30T10:00:02.100Z");
        let mut alias = submitted.clone();
        alias["startedAt"] = json!("2026-09-30T10:00:00.100000124Z");
        timer_no_effect_request(
            &fixture,
            &admin,
            &[actor],
            &[task_id],
            app.clone(),
            &format!("{base}/timer/history"),
            alias,
            &cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let replay = timer_no_effect_request(
            &fixture,
            &admin,
            &[actor],
            &[task_id],
            app.clone(),
            &format!("{base}/timer/history"),
            submitted.clone(),
            &cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replay, created);
        let after_audit: Value = sqlx::query_scalar(
            "SELECT after_value FROM fvoci.task_timer_audit WHERE user_id=$1 AND request_id=$2",
        )
        .bind(actor)
        .bind(Uuid::parse_str(submitted["requestId"].as_str().unwrap()).unwrap())
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(after_audit["submittedStartedAt"], submitted["startedAt"]);
        assert_eq!(after_audit["submittedEndedAt"], submitted["endedAt"]);
        assert_eq!(after_audit["startedAt"], created["record"]["startedAt"]);
        assert_eq!(after_audit["endedAt"], created["record"]["endedAt"]);
        // Controlled historical034 row; current ordinary HTTP parsing already
        // normalizes NEW input to milliseconds. Never rewrite this raw fixture.
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,'2026-09-30T11:00:00.100999Z','2026-09-30T11:00:02.100999Z',2,'old microsecond anchor')")
            .bind(id).bind(workspace).bind(task_id).bind(actor).execute(&admin).await.unwrap();
        let (status, entries) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/time-entries"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{entries}");
        let legacy = entries["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(id))
            .unwrap()
            .clone();
        let raw_before: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1")
                .bind(id)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(legacy["startedAt"], "2026-09-30T11:00:00.100999Z");
        let (status, history) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/timer/history?from=2026-09-30&to=2026-09-30"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{history}");
        let baseline = history["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == legacy["id"])
            .unwrap();
        assert_eq!(baseline["startedAt"], legacy["startedAt"]);
        let correction = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":baseline["revision"],"expectedStartedAt":baseline["startedAt"],"expectedEndedAt":baseline["endedAt"],"expectedNote":baseline["note"],"startedAt":"2026-09-30T11:00:00.100000123Z","endedAt":"2026-09-30T11:00:00.600000456Z","note":"Explicit half-second correction","reason":"Preserve old baseline, replace effective range"})).await;
        let path = format!("{base}/timer/records/{id}/correct");
        let (status, corrected) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &path,
            Some(correction.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "raw expected CAS is not normalized: {corrected}"
        );
        assert_eq!(corrected["record"]["startedAt"], "2026-09-30T11:00:00.100Z");
        assert_eq!(corrected["record"]["endedAt"], "2026-09-30T11:00:00.600Z");
        let raw_after: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1")
                .bind(id)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(raw_after, raw_before, "every old034 column unchanged");
        let (before_audit, after_audit): (Value,Value) = sqlx::query_as("SELECT before_value,after_value FROM fvoci.task_timer_audit WHERE user_id=$1 AND request_id=$2")
            .bind(actor).bind(Uuid::parse_str(correction["requestId"].as_str().unwrap()).unwrap()).fetch_one(&admin).await.unwrap();
        assert_eq!(before_audit["startedAt"], baseline["startedAt"]);
        assert_eq!(before_audit["endedAt"], baseline["endedAt"]);
        assert_eq!(after_audit["submittedStartedAt"], correction["startedAt"]);
        assert_eq!(after_audit["submittedEndedAt"], correction["endedAt"]);
        assert_eq!(after_audit["startedAt"], corrected["record"]["startedAt"]);
        assert_eq!(after_audit["endedAt"], corrected["record"]["endedAt"]);
        let mut alias = correction.clone();
        alias["endedAt"] = json!("2026-09-30T11:00:00.600000457Z");
        timer_no_effect_request(
            &fixture,
            &admin,
            &[actor],
            &[task_id],
            app.clone(),
            &path,
            alias,
            &cookie,
            StatusCode::CONFLICT,
        )
        .await;
        let replay = timer_no_effect_request(
            &fixture,
            &admin,
            &[actor],
            &[task_id],
            app.clone(),
            &path,
            correction,
            &cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replay, corrected);
        let (status, fresh) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/timer/history?from=2026-09-30&to=2026-09-30"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        assert_eq!(
            fresh["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == legacy["id"])
                .unwrap(),
            &corrected["record"]
        );
        let (status, entries) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/time-entries"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{entries}");
        let effective = entries["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == legacy["id"])
            .unwrap();
        assert_eq!(effective["durationSeconds"], 0);
        assert_eq!(effective["startedAt"], corrected["record"]["startedAt"]);
        assert_eq!(effective["endedAt"], corrected["record"]["endedAt"]);
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn timer_summary_empty_paused_idle_and_zero_segment_day_are_exact() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "ZERODAY", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Paused reservation"}),
        )
        .await;
        let base = format!(
            "/api/v1/workspaces/{workspace}/tasks/{}/timer",
            task["id"].as_str().unwrap()
        );
        sqlx::query("UPDATE fvoci.users SET timezone='Asia/Seoul' WHERE id=$1")
            .bind(actor)
            .execute(&admin)
            .await
            .unwrap();
        let path = format!("{base}/summary?from=2026-09-30&to=2026-10-01");
        let (status, empty) =
            timer_checked_request(&fixture, app.clone(), "GET", &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{empty}");
        assert_eq!(empty["totalMilliseconds"], 0);
        assert_eq!(empty["unfinished"], false);
        for day in empty["days"].as_array().unwrap() {
            assert_eq!(day["milliseconds"], 0);
        }
        let start = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null})).await;
        let (status, started) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &base,
            Some(start),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let pause = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"operation":"pause","expectedVersion":1,"runId":started["runId"]})).await;
        let (status, paused) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &base,
            Some(pause),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{paused}");
        let (status, state) =
            timer_checked_request(&fixture, app.clone(), "GET", &base, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{state}");
        assert_eq!(state["run"]["status"], "paused");
        // Include both actual local dates if a legitimate start/pause crosses
        // midnight; never assume the wall-clock day is one closed day.
        let (from_day, to_day): (String, String) = sqlx::query_as(
            "SELECT (min(started_at) AT TIME ZONE 'Asia/Seoul')::date::text,(max(ended_at) AT TIME ZONE 'Asia/Seoul')::date::text FROM fvoci.task_timer_segments WHERE run_id=$1",
        ).bind(Uuid::parse_str(started["runId"].as_str().unwrap()).unwrap())
        .fetch_one(&admin).await.unwrap();
        let today = format!("{base}/summary?from={from_day}&to={to_day}");
        let (status, first) =
            timer_checked_request(&fixture, app.clone(), "GET", &today, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let (status, second) =
            timer_checked_request(&fixture, app.clone(), "GET", &today, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(first["unfinished"], true);
        assert_eq!(second["unfinished"], true);
        assert_eq!(
            first["totalMilliseconds"],
            state["run"]["elapsedMilliseconds"]
        );
        assert_eq!(
            second["totalMilliseconds"], first["totalMilliseconds"],
            "paused idle never accumulates"
        );
        // A controlled zero-length closed fixture verifies discovery only. It
        // does not claim an actual server-clock rollback or synthetic clock.
        let zero_task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Zero anchor fixture"}),
        )
        .await;
        let zero_id = Uuid::parse_str(zero_task["id"].as_str().unwrap()).unwrap();
        let zero_run = Uuid::now_v7();
        let zero_segment = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at,stopped_at) VALUES($1,$2,$3,$4,'stopped',2,'2026-09-30T15:00:00Z','2026-09-30T15:00:00Z')")
            .bind(zero_run).bind(actor).bind(workspace).bind(zero_id).execute(&admin).await.unwrap();
        sqlx::query("INSERT INTO fvoci.task_timer_segments(id,run_id,user_id,workspace_id,task_id,started_at,ended_at) VALUES($1,$2,$3,$4,$5,'2026-09-30T15:00:00Z','2026-09-30T15:00:00Z')")
            .bind(zero_segment).bind(zero_run).bind(actor).bind(workspace).bind(zero_id).execute(&admin).await.unwrap();
        let zero_base = format!("/api/v1/workspaces/{workspace}/tasks/{zero_id}/timer");
        let before = timer_effects(&admin, &[actor], &[zero_id]).await;
        for (day, count) in [("2026-09-30", 0), ("2026-10-01", 1), ("2026-10-02", 0)] {
            let (status, history) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{zero_base}/history?from={day}&to={day}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{history}");
            assert_eq!(
                history["items"].as_array().unwrap().len(),
                count,
                "zero anchor only on its local day"
            );
            if count == 1 {
                assert_eq!(history["items"][0]["id"], json!(zero_segment));
            }
            let (status, summary) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{zero_base}/summary?from={day}&to={day}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{summary}");
            assert_eq!(summary["totalMilliseconds"], 0);
            assert_eq!(summary["unfinished"], false);
        }
        assert_eq!(
            timer_effects(&admin, &[actor], &[zero_id]).await,
            before,
            "reads cannot mutate controlled zero fixture"
        );
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    // Store each completed phase before a later fixture prerequisite can fail.
    // Evidence is optional outside the coordinator's explicit runtime batch.
    fn timer_measurement_receipt(name: &str, value: &Value) {
        if let Ok(directory) = std::env::var("FVOCI_W5_EVIDENCE_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(name))
                .unwrap();
            use std::io::Write;
            let release_complete = name == "history-null-locator-release.json"
                || name == "history-explain-101-2048.json";
            let checkpoint = json!({"checkpoint":name,"observationsCaptured":true,
                "legacyRelease":if release_complete {"ASSERTIONS_PASSED"} else {"NOTREACHED"},
                "finalResourceCleanup":"NOTREACHED; see actual runner result", "observations":value});
            file.write_all(
                serde_json::to_string_pretty(&checkpoint)
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
            file.sync_all().unwrap();
        }
    }

    async fn timer_measure_history_sql(
        fixture: &TimerFixture,
        workspace: Uuid,
        task: Uuid,
        actor: Uuid,
        with_witness: bool,
        analyze: Option<bool>,
    ) -> Value {
        let sql = fvoci_server::db::task_timer::history_measurement_sql(with_witness);
        let sql = match analyze {
            None => sql,
            Some(false) => format!("EXPLAIN (FORMAT JSON) {sql}"),
            Some(true) => format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"),
        };
        fixture.probe("before measured read").await;
        let mut tx = fvoci_server::db::context::begin_read(&fixture.pool)
            .await
            .unwrap();
        fvoci_server::db::context::set_tenant(&mut tx, workspace)
            .await
            .unwrap();
        fvoci_server::db::context::set_self_user(&mut tx, actor)
            .await
            .unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        // Reuse exact production binds. No session/system setting, planner knob,
        // pool growth, copied record query, or diagnostic HTTP endpoint.
        let rows = sqlx::query(&sql)
            .bind(workspace)
            .bind(task)
            .bind(actor)
            .bind(date)
            .bind(date)
            .bind("Asia/Seoul")
            .bind(Option::<chrono::DateTime<chrono::Utc>>::None)
            .bind(Option::<Uuid>::None)
            .bind(Option::<String>::None)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        use sqlx::Row;
        let value = if analyze.is_some() {
            rows[0].get::<Value, _>(0)
        } else if with_witness {
            json!({"version":rows[0].get::<Vec<i64>,_>(0),"records":rows[0].get::<Value,_>(1)})
        } else {
            json!({"count":rows.len()})
        };
        tx.commit().await.unwrap();
        fixture.probe("after measured read").await;
        value
    }

    #[tokio::test]
    async fn timer_history_exact_queries_explain_actual_role_101_and_2048() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        sqlx::query("UPDATE fvoci.users SET timezone='Asia/Seoul' WHERE id=$1")
            .bind(actor)
            .execute(&admin)
            .await
            .unwrap();
        let project = create_project(app.clone(), &cookie, workspace, "PLANSQL", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Measured personal history"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let other_task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Unrelated task fixture"}),
        )
        .await;
        let other_task_id = Uuid::parse_str(other_task["id"].as_str().unwrap()).unwrap();
        // Unrelated actor data only; no login/password/session credential injection.
        let other_actor = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.users(id,email,given_name) VALUES($1,$2,'Unrelated measurement actor')")
            .bind(other_actor).bind(format!("measurement-{other_actor}@example.com")).execute(&admin).await.unwrap();
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}");
        for _ in 0..101 {
            let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T10:00:00Z","endedAt":"2026-09-30T10:00:30Z","note":null,"reason":"Actual API measurement baseline"})).await;
            let (status, value) = timer_checked_request(
                &fixture,
                app.clone(),
                "POST",
                &format!("{base}/timer/history"),
                Some(body),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        let mut evidence = Vec::new();
        for count in [101_i64, 2048] {
            if count == 2048 {
                // Bulk controlled historical data measures size/distribution;
                // these are not claimed as actual API effects or clock commands.
                for (fixture_actor, fixture_task, n) in [
                    (actor, task_id, 1947_i64),
                    (other_actor, task_id, 2048),
                    (actor, other_task_id, 2048),
                ] {
                    let mut tx = admin.begin().await.unwrap();
                    for _ in 0..n {
                        let entry = Uuid::now_v7();
                        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,'2026-09-30T10:00:00Z','2026-09-30T10:00:30Z',30,'Controlled size fixture')")
                            .bind(entry).bind(workspace).bind(fixture_task).bind(fixture_actor).execute(&mut *tx).await.unwrap();
                        sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason) VALUES($1,$2,$3,$4,$5,$6,'time.manual','null'::jsonb,$7,'Controlled size fixture only')")
                            .bind(Uuid::now_v7()).bind(fixture_actor).bind(Uuid::now_v7()).bind(workspace).bind(fixture_task).bind(entry)
                            .bind(json!({"recordId":entry,"kind":"manual","startedAt":"2026-09-30T10:00:00Z","endedAt":"2026-09-30T10:00:30Z","note":"Controlled size fixture","revision":0}))
                            .execute(&mut *tx).await.unwrap();
                    }
                    tx.commit().await.unwrap();
                }
            }
            // Statistics on this discardable owned fixture only. App queries
            // keep normal settings; no production settings/index/schema changes.
            for table in [
                "time_entries",
                "task_timer_audit",
                "task_timer_segments",
                "task_timer_legacy_open",
                "task_timer_runs",
            ] {
                sqlx::query(&format!("ANALYZE fvoci.{table}"))
                    .execute(&admin)
                    .await
                    .unwrap();
            }
            let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.time_entries WHERE user_id=$1 AND workspace_id=$2 AND task_id=$3")
                .bind(actor).bind(workspace).bind(task_id).fetch_one(&admin).await.unwrap();
            assert_eq!(actual, count);
            let pg_version: String = sqlx::query_scalar("SHOW server_version")
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
            for candidate in [false, true] {
                let plan = timer_measure_history_sql(
                    &fixture,
                    workspace,
                    task_id,
                    actor,
                    candidate,
                    Some(false),
                )
                .await;
                timer_measurement_receipt(
                    &format!("history-plan-{count}-{candidate}-explain.json"),
                    &json!({"personalRows":count,"candidate":candidate,"backend":fixture.pid,"plan":plan}),
                );
                // First/warm describe observed order, not OS cache eviction.
                let first = timer_measure_history_sql(
                    &fixture,
                    workspace,
                    task_id,
                    actor,
                    candidate,
                    Some(true),
                )
                .await;
                timer_measurement_receipt(
                    &format!("history-plan-{count}-{candidate}-first.json"),
                    &json!({"personalRows":count,"candidate":candidate,"backend":fixture.pid,"plan":first}),
                );
                let warm = timer_measure_history_sql(
                    &fixture,
                    workspace,
                    task_id,
                    actor,
                    candidate,
                    Some(true),
                )
                .await;
                timer_measurement_receipt(
                    &format!("history-plan-{count}-{candidate}-warm.json"),
                    &json!({"personalRows":count,"candidate":candidate,"backend":fixture.pid,"plan":warm}),
                );
                let result =
                    timer_measure_history_sql(&fixture, workspace, task_id, actor, candidate, None)
                        .await;
                let receipt = json!({"personalRows":count,"unrelatedActorRows":if count==2048 {2048}else{0},"unrelatedTaskRows":if count==2048 {2048}else{0},"postgres":pg_version,"candidateNotAdopted":candidate,"actualRouterBackend":fixture.pid,"plan":plan,"firstObserved":first,"warmObserved":warm,"queryResult":result,"limits":"Warm/first execution order only; no cold cache claim; internal Router/app-role queries, no production latency promise"});
                timer_measurement_receipt(
                    &format!(
                        "history-explain-{count}-{}.json",
                        if candidate { "candidate" } else { "current" }
                    ),
                    &receipt,
                );
                println!(
                    "W5 EXPLAIN rows={count} candidate={candidate} firstMs={} warmMs={}",
                    receipt["firstObserved"][0]["Execution Time"],
                    receipt["warmObserved"][0]["Execution Time"]
                );
                evidence.push(receipt);
                if candidate {
                    assert_eq!(result["version"], json!([count, count, 0, 0]));
                    assert_eq!(result["records"].as_array().unwrap().len(), 101);
                    let allowed: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM fvoci.time_entries WHERE user_id=$1 AND workspace_id=$2 AND task_id=$3")
                        .bind(actor).bind(workspace).bind(task_id).fetch_all(&admin).await.unwrap();
                    for row in result["records"].as_array().unwrap() {
                        assert!(
                            allowed
                                .contains(&Uuid::parse_str(row["id"].as_str().unwrap()).unwrap()),
                            "no unrelated actor/task row"
                        );
                    }
                } else {
                    assert_eq!(result["count"], 101);
                }
            }
            fixture.probe("before endpoint timing").await;
            let clock = std::time::Instant::now();
            let (status, history) = json_request(
                app.clone(),
                "GET",
                &format!("{base}/timer/history?from=2026-09-30&to=2026-09-30"),
                None,
                Some(&cookie),
            )
            .await;
            let elapsed = clock.elapsed().as_secs_f64() * 1000.0;
            fixture.probe("after endpoint timing").await;
            let receipt = json!({"personalRows":count,"actualCurrentEndpointMilliseconds":elapsed,"includes":"HTTP auth +pool acquires +permission +SQL +serialization; excludes fixture probes; no mocked transport/nativeTCP"});
            timer_measurement_receipt(&format!("history-endpoint-{count}.json"), &receipt);
            evidence.push(receipt);
            assert_eq!(status, StatusCode::OK, "{history}");
            assert_eq!(history["items"].as_array().unwrap().len(), 100);
        }
        // Actual oldAPI open +explicit NULLlocator release verifies the fourth
        // component catches reservedLegacy changes the first three miss.
        let (status, open) = timer_checked_request(&fixture, app.clone(),"POST",&format!("{base}/time-entries"),Some(json!({"startedAt":"2026-09-30T09:00:00Z","note":"Explicit unresolved legacy release"})),Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED, "{open}");
        let before =
            timer_measure_history_sql(&fixture, workspace, task_id, actor, true, None).await;
        let raw: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1")
                .bind(Uuid::parse_str(open["id"].as_str().unwrap()).unwrap())
                .fetch_one(&admin)
                .await
                .unwrap();
        let release = captured(
            app.clone(),
            &cookie,
            json!({"requestId":Uuid::now_v7(),"timeEntryId":open["id"]}),
        )
        .await;
        let (status, released) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            "/api/v1/me/task-timer/legacy-release",
            Some(release.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{released}");
        let after =
            timer_measure_history_sql(&fixture, workspace, task_id, actor, true, None).await;
        assert_eq!(before["version"], json!([2048, 2049, 0, 1]));
        assert_eq!(after["version"], json!([2048, 2049, 0, 0]));
        let raw_after: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1")
                .bind(Uuid::parse_str(open["id"].as_str().unwrap()).unwrap())
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(raw_after, raw);
        let replay = timer_no_effect_request(
            &fixture,
            &admin,
            &[actor],
            &[task_id],
            app.clone(),
            "/api/v1/me/task-timer/legacy-release",
            release,
            &cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replay, released);
        evidence.push(json!({"nullLocatorRelease":{"beforeVersion":before["version"],"afterVersion":after["version"],"raw034Unchanged":true},"fourCountWitness":"EXPERIMENT_NOTADOPTED","productionCursor":"EFFECTIVE_RECORD_FINGERPRINT"}));
        timer_measurement_receipt(
            "history-null-locator-release.json",
            evidence.last().unwrap(),
        );
        timer_measurement_receipt("history-explain-101-2048.json", &json!(evidence));
        for result in &evidence {
            if result.get("firstObserved").is_some() {
                println!(
                    "W5 EXPLAIN rows={} candidate={} firstMs={} warmMs={}",
                    result["personalRows"],
                    result["candidateNotAdopted"],
                    result["firstObserved"][0]["Execution Time"],
                    result["warmObserved"][0]["Execution Time"]
                );
            }
        }
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    fn history_continuation(path: &str, cursor: &str) -> String {
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("cursor", cursor)
            .finish();
        format!("{path}&{encoded}")
    }

    async fn history_read_without_effects(
        fixture: &TimerFixture,
        admin: &sqlx::PgPool,
        actors: &[Uuid],
        tasks: &[Uuid],
        app: axum::Router,
        path: &str,
        cookie: &str,
    ) -> (StatusCode, Value) {
        let before = timer_effects(admin, actors, tasks).await;
        let response = timer_checked_request(fixture, app, "GET", path, None, Some(cookie)).await;
        assert_eq!(
            timer_effects(admin, actors, tasks).await,
            before,
            "history success/denial cannot change full ordered effects"
        );
        response
    }

    async fn seed_equal_history(
        fixture: &TimerFixture,
        app: axum::Router,
        cookie: &str,
        base: &str,
        start: &str,
        end: &str,
        count: usize,
    ) -> Vec<Value> {
        let capture = captured(app.clone(), cookie, json!({})).await;
        let mut records = Vec::new();
        for _ in 0..count {
            let body = json!({"expectedActorId":capture["expectedActorId"],
                "expectedSessionId":capture["expectedSessionId"],"requestId":Uuid::now_v7(),
                "startedAt":start,"endedAt":end,"note":null,"reason":"Equal-anchor snapshot fixture"});
            let (status, response) = timer_checked_request(
                fixture,
                app.clone(),
                "POST",
                &format!("{base}/history"),
                Some(body),
                Some(cookie),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{response}");
            records.push(response["record"].clone());
        }
        records
    }

    #[tokio::test]
    async fn timer_history_snapshot_stable_context_equal_count_and_empty_header() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let project = create_project(app.clone(), &cookie, workspace, "SNAP", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Content snapshot"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let other_task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Other task"}),
        )
        .await;
        let other_task_id = Uuid::parse_str(other_task["id"].as_str().unwrap()).unwrap();
        let other = add_workspace_user(&admin, workspace, "member", "snapshot-other").await;
        add_project_member(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            other.user_id,
            "viewer",
        )
        .await;
        let actors = [actor, other.user_id];
        let tasks = [task_id, other_task_id];
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}/timer");
        let mut seeded = seed_equal_history(
            &fixture,
            app.clone(),
            &cookie,
            &base,
            "2026-09-30T10:00:00Z",
            "2026-09-30T10:00:30Z",
            101,
        )
        .await;
        seeded.sort_by(|a, b| b["id"].as_str().unwrap().cmp(a["id"].as_str().unwrap()));
        let path = format!("{base}/history?from=2026-09-30&to=2026-09-30");
        let (status, first) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &path,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["items"], json!(&seeded[..100]));
        let cursor = first["nextCursor"].as_str().unwrap();
        let next = history_continuation(&path, cursor);
        let (status, tail) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tail}");
        assert_eq!(tail["items"], json!(&seeded[100..]));
        assert!(tail["nextCursor"].is_null());
        // Controlled unrelated historical rows are not claimed as API writers.
        for (row_actor, row_task) in [(other.user_id, task_id), (actor, other_task_id)] {
            sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,'2026-09-30T10:00:00Z','2026-09-30T10:00:30Z',30,'Unrelated private record')")
                .bind(Uuid::now_v7()).bind(workspace).bind(row_task).bind(row_actor)
                .execute(&admin).await.unwrap();
        }
        let (status, stable) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{stable}");
        assert_eq!(stable["items"], tail["items"]);
        for wrong_path in [
            history_continuation(&format!("{base}/history?from=2026-09-29&to=2026-09-30"),cursor),
            history_continuation(&format!("/api/v1/workspaces/{workspace}/tasks/{other_task_id}/timer/history?from=2026-09-30&to=2026-09-30"),cursor),
            history_continuation(&path,&"x".repeat(4097)),
        ] {
            let (status, response) = history_read_without_effects(&fixture,&admin,&actors,&tasks,
                app.clone(),&wrong_path,&cookie).await;
            assert_eq!(status,StatusCode::BAD_REQUEST,"{response}");
        }
        use base64::Engine;
        let decoded: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(cursor)
                .unwrap(),
        )
        .unwrap();
        for (field, value) in [
            ("version", json!(2)),
            ("workspace", json!(Uuid::now_v7())),
            ("fingerprint", json!("not-a-sha256-witness")),
            ("unknownField", json!(true)),
        ] {
            let mut malformed = decoded.clone();
            malformed[field] = value;
            let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&malformed).unwrap());
            let (status, response) = history_read_without_effects(
                &fixture,
                &admin,
                &actors,
                &tasks,
                app.clone(),
                &history_continuation(&path, &token),
                &cookie,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
        }
        let (status, response) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &other.cookie,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        sqlx::query("UPDATE fvoci.users SET timezone='America/New_York' WHERE id=$1")
            .bind(actor)
            .execute(&admin)
            .await
            .unwrap();
        let (status, response) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        sqlx::query("UPDATE fvoci.users SET timezone='Asia/Seoul' WHERE id=$1")
            .bind(actor)
            .execute(&admin)
            .await
            .unwrap();
        // Same IDs/counts, different content: controlled restore-like replacement.
        // This is not an end-to-end native restore or an authorized ordinary edit.
        let raw_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.time_entries WHERE user_id=$1 AND task_id=$2",
        )
        .bind(actor)
        .bind(task_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        sqlx::query("UPDATE fvoci.time_entries SET note='' WHERE id=$1")
            .bind(Uuid::parse_str(seeded[0]["id"].as_str().unwrap()).unwrap())
            .execute(&admin)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.time_entries WHERE user_id=$1 AND task_id=$2"
            )
            .bind(actor)
            .bind(task_id)
            .fetch_one(&admin)
            .await
            .unwrap(),
            raw_count
        );
        let (status, changed) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{changed}");
        assert_eq!(changed["params"]["code"], "timer_history_changed");
        sqlx::query("UPDATE fvoci.time_entries SET note=NULL WHERE id=$1")
            .bind(Uuid::parse_str(seeded[0]["id"].as_str().unwrap()).unwrap())
            .execute(&admin)
            .await
            .unwrap();
        let (status, identical) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "identical content restoration: {identical}"
        );
        assert_eq!(identical["items"], tail["items"]);
        let (status, open) = timer_checked_request(&fixture, app.clone(), "POST",
            &format!("/api/v1/workspaces/{workspace}/tasks/{task_id}/time-entries"),
            Some(json!({"startedAt":"2026-09-30T09:00:00Z","note":"Explicit unresolved legacy range"})),
            Some(&cookie)).await;
        assert_eq!(status, StatusCode::CREATED, "{open}");
        let raw_open: Value =
            sqlx::query_scalar("SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1")
                .bind(Uuid::parse_str(open["id"].as_str().unwrap()).unwrap())
                .fetch_one(&admin)
                .await
                .unwrap();
        let (status, reserved) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &path,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reserved}");
        let release_cursor = history_continuation(&path, reserved["nextCursor"].as_str().unwrap());
        let release = captured(
            app.clone(),
            &cookie,
            json!({"requestId":Uuid::now_v7(),"timeEntryId":open["id"]}),
        )
        .await;
        let (status, released) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            "/api/v1/me/task-timer/legacy-release",
            Some(release.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{released}");
        let (status, changed) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &release_cursor,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{changed}");
        assert_eq!(changed["params"]["code"], "timer_history_changed");
        assert_eq!(
            sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(e) FROM fvoci.time_entries e WHERE id=$1"
            )
            .bind(Uuid::parse_str(open["id"].as_str().unwrap()).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap(),
            raw_open
        );
        let replay = timer_no_effect_request(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            "/api/v1/me/task-timer/legacy-release",
            release,
            &cookie,
            StatusCode::OK,
        )
        .await;
        assert_eq!(replay, released);
        // Empty header must still reject a changed saved continuation.
        sqlx::query("DELETE FROM fvoci.time_entries WHERE user_id=$1 AND task_id=$2")
            .bind(actor)
            .bind(task_id)
            .execute(&admin)
            .await
            .unwrap();
        let (status, empty_changed) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{empty_changed}");
        assert_eq!(empty_changed["params"]["code"], "timer_history_changed");
        let (status, empty) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &path,
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{empty}");
        assert_eq!(empty["items"], json!([]));
        assert!(empty["nextCursor"].is_null());
        println!("W5 snapshot stable101/100+1, unrelated stable, context400, NULL-vs-empty samecount409, identical restore200, empty-header409/read0; all reads no effects");
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    #[tokio::test]
    async fn timer_history_snapshot_public_sessions_revocation_and_null_cleanup_notes() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, owner_cookie, owner, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let actor = add_workspace_user(&admin, workspace, "member", "history-real-login").await;
        let email: String = sqlx::query_scalar("SELECT email FROM fvoci.users WHERE id=$1")
            .bind(actor.user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
        let password = "history-fixture-password1";
        let password_hash =
            fvoci_server::auth::password::hash_password(password, &fixture.password_keys)
                .await
                .unwrap();
        sqlx::query("UPDATE fvoci.users SET password_hash=$2 WHERE id=$1")
            .bind(actor.user_id)
            .bind(password_hash)
            .execute(&admin)
            .await
            .unwrap();
        let (s1, me1) = fixture.login(app.clone(), &email, password).await;
        assert_eq!(me1["userId"], json!(actor.user_id));
        let project =
            create_project(app.clone(), &owner_cookie, workspace, "SESSIONH", "private").await;
        let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
        add_project_member(
            app.clone(),
            &owner_cookie,
            workspace,
            project["id"].as_str().unwrap(),
            actor.user_id,
            "member",
        )
        .await;
        let task = create_task(
            app.clone(),
            &owner_cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Current session and private cleanup notes"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let actors = [owner, actor.user_id];
        let tasks = [task_id];
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}/timer");
        let (status, started) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &base,
            Some(
                captured(
                    app.clone(),
                    &s1,
                    json!({"requestId":Uuid::now_v7(),"operation":"start",
                "expectedVersion":0,"runId":null,"note":"Actual running note"}),
                )
                .await,
            ),
            Some(&s1),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let at = chrono::DateTime::parse_from_rfc3339(started["serverNow"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
        let start = (at - chrono::Duration::hours(2)).to_rfc3339();
        let end = (at - chrono::Duration::hours(2) + chrono::Duration::seconds(30)).to_rfc3339();
        seed_equal_history(&fixture, app.clone(), &s1, &base, &start, &end, 101).await;
        let from = at.date_naive() - chrono::Duration::days(1);
        let to = at.date_naive() + chrono::Duration::days(1);
        let path = format!("{base}/history?from={from}&to={to}");
        let (status, first) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &path,
            &s1,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        let next = history_continuation(&path, first["nextCursor"].as_str().unwrap());
        let (s2, me2) = fixture.login(app.clone(), &email, password).await;
        assert_eq!(me2["userId"], me1["userId"]);
        assert_ne!(me2["sessionId"], me1["sessionId"]);
        let (status, response) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &s2,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{response}");
        assert_eq!(response["params"]["code"], "timer_context_changed");
        let (status, stable) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &s1,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{stable}");
        assert_eq!(stable["items"].as_array().unwrap().len(), 2);
        // Revocation is committed before the next read. Current ACL wins before cursor checks.
        sqlx::query("DELETE FROM fvoci.project_members WHERE project_id=$1 AND user_id=$2")
            .bind(project_id)
            .bind(actor.user_id)
            .execute(&admin)
            .await
            .unwrap();
        let (status, denied) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &s1,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
        let cleanup = captured(
            app.clone(),
            &s2,
            json!({"requestId":Uuid::now_v7(),
            "runId":started["runId"],"expectedVersion":started["version"]}),
        )
        .await;
        let (status, closed) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            "/api/v1/me/task-timer/stop",
            Some(cleanup),
            Some(&s2),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{closed}");
        let run_id = Uuid::parse_str(started["runId"].as_str().unwrap()).unwrap();
        let (segment_id, projection): (Uuid, Option<Uuid>) = sqlx::query_as(
            "SELECT id,time_entry_id FROM fvoci.task_timer_segments WHERE run_id=$1",
        )
        .bind(run_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            projection.is_none(),
            "revoked self cleanup cannot project hidden034"
        );
        let audit: Value = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM fvoci.task_timer_audit a WHERE user_id=$1 AND verb='cleanup'",
        )
        .bind(actor.user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(audit["workspace_id"].is_null() && audit["task_id"].is_null());
        assert_eq!(audit["after_value"]["recordId"], json!(segment_id));
        assert_eq!(audit["after_value"]["note"], "Actual running note");
        add_project_member(
            app.clone(),
            &owner_cookie,
            workspace,
            project["id"].as_str().unwrap(),
            actor.user_id,
            "viewer",
        )
        .await;
        let (status, changed) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &s1,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{changed}");
        assert_eq!(changed["params"]["code"], "timer_history_changed");
        // Controlled fallback poison proves the real NULL-locator audit remains the winner.
        sqlx::query(
            "UPDATE fvoci.task_timer_runs SET note='Controlled mutable fallback' WHERE id=$1",
        )
        .bind(run_id)
        .execute(&admin)
        .await
        .unwrap();
        let (status, fresh) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &path,
            &s2,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fresh}");
        let segment = fresh["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(segment_id))
            .unwrap();
        assert_eq!(segment["kind"], "segment");
        assert_eq!(segment["note"], "Actual running note");
        assert!(segment["endedAt"].is_string());
        let (status, _) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            "/api/v1/auth/logout",
            None,
            Some(&s1),
        )
        .await;
        assert!(status.is_success());
        let (status, logged_out) = history_read_without_effects(
            &fixture,
            &admin,
            &actors,
            &tasks,
            app.clone(),
            &next,
            &s1,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{logged_out}");
        println!("W5 snapshot actual publicS1/S2/current-session409, current ACL404 before cursor, real NULL-locator cleanup note preserved, logout401; all reads no effects");
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }

    async fn measure_snapshot_statement(
        fixture: &TimerFixture,
        workspace: Uuid,
        task: Uuid,
        actor: Uuid,
        analyze: Option<bool>,
    ) -> Value {
        let sql = fvoci_server::db::task_timer::history_snapshot_measurement_sql();
        let sql = match analyze {
            None => sql,
            Some(false) => format!("EXPLAIN (FORMAT JSON) {sql}"),
            Some(true) => format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"),
        };
        fixture.probe("before exact snapshot statement").await;
        let mut tx = fvoci_server::db::context::begin_read(&fixture.pool)
            .await
            .unwrap();
        fvoci_server::db::context::set_tenant(&mut tx, workspace)
            .await
            .unwrap();
        fvoci_server::db::context::set_self_user(&mut tx, actor)
            .await
            .unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        let rows = sqlx::query(&sql)
            .bind(workspace)
            .bind(task)
            .bind(actor)
            .bind(date)
            .bind(date)
            .bind("Asia/Seoul")
            .bind(Option::<chrono::DateTime<chrono::Utc>>::None)
            .bind(Option::<Uuid>::None)
            .bind(Option::<String>::None)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        use sqlx::Row;
        let value = if analyze.is_some() {
            rows[0].get::<Value, _>(0)
        } else {
            json!({"fingerprint":rows[0].get::<String,_>(0),
                "ids":rows.iter().filter_map(|r|r.get::<Option<Uuid>,_>(1)).collect::<Vec<_>>()})
        };
        tx.commit().await.unwrap();
        fixture.probe("after exact snapshot statement").await;
        value
    }

    #[tokio::test]
    async fn timer_history_snapshot_exact_production_plans_101_2048_and_mixed3072() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        sqlx::query("UPDATE fvoci.users SET timezone='Asia/Seoul' WHERE id=$1")
            .bind(actor)
            .execute(&admin)
            .await
            .unwrap();
        let project = create_project(app.clone(), &cookie, workspace, "SNAPPLAN", "private").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Exact production statement plans"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let other_task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Unrelated plan task"}),
        )
        .await;
        let other_task_id = Uuid::parse_str(other_task["id"].as_str().unwrap()).unwrap();
        let other_actor = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO fvoci.users(id,email,given_name) VALUES($1,$2,'Unrelated snapshot actor')",
        )
        .bind(other_actor)
        .bind(format!("snapshot-{other_actor}@example.com"))
        .execute(&admin)
        .await
        .unwrap();
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}/timer");
        seed_equal_history(
            &fixture,
            app.clone(),
            &cookie,
            &base,
            "2026-09-30T10:00:00Z",
            "2026-09-30T10:00:30Z",
            101,
        )
        .await;
        let path = format!("{base}/history?from=2026-09-30&to=2026-09-30");
        let mut original_fingerprint = None;
        for stage in ["101", "2048", "mixed3072"] {
            if stage == "2048" {
                // Same controlled raw/audit size/distribution as the14ba baseline.
                for (row_actor, row_task, count) in [
                    (actor, task_id, 1947),
                    (other_actor, task_id, 2048),
                    (actor, other_task_id, 2048),
                ] {
                    let mut tx = admin.begin().await.unwrap();
                    for _ in 0..count {
                        let id = Uuid::now_v7();
                        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,'2026-09-30T10:00:00Z','2026-09-30T10:00:30Z',30,NULL)")
                            .bind(id).bind(workspace).bind(row_task).bind(row_actor).execute(&mut *tx).await.unwrap();
                        sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason) VALUES($1,$2,$3,$4,$5,$6,'time.create','{}',$7,'Controlled historical size fixture')")
                            .bind(Uuid::now_v7()).bind(row_actor).bind(Uuid::now_v7()).bind(workspace)
                            .bind(row_task).bind(id).bind(json!({"recordId":id,"kind":"manual","revision":0}))
                            .execute(&mut *tx).await.unwrap();
                    }
                    tx.commit().await.unwrap();
                }
            }
            if stage == "mixed3072" {
                // Actual ordinary server start/stop establishes the canonical run.
                let (status,started)=timer_request(app.clone(),"POST",&base,
                    Some(json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"Actual mixed run"})),Some(&cookie)).await;
                assert_eq!(status, StatusCode::OK, "{started}");
                let (status,stopped)=timer_request(app.clone(),"POST",&base,
                    Some(json!({"requestId":Uuid::now_v7(),"operation":"stop","expectedVersion":started["version"],"runId":started["runId"],"note":"Actual stop note"})),Some(&cookie)).await;
                assert_eq!(status, StatusCode::OK, "{stopped}");
                let run_id = Uuid::parse_str(started["runId"].as_str().unwrap()).unwrap();
                // Explicit controlled historical canonical ranges/audits measure
                // a mixed distribution, not actual1024 clock or native-restore effects.
                let mut tx = admin.begin().await.unwrap();
                for n in 0..1024 {
                    let id = Uuid::now_v7();
                    sqlx::query("INSERT INTO fvoci.task_timer_segments(id,run_id,user_id,workspace_id,task_id,started_at,ended_at) VALUES($1,$2,$3,$4,$5,'2026-09-30T09:00:00Z','2026-09-30T09:00:00.750Z')")
                        .bind(id).bind(run_id).bind(actor).bind(workspace).bind(task_id)
                        .execute(&mut *tx).await.unwrap();
                    // NULL locator notes must join the actual scoped segments.
                    sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,verb,before_value,after_value,reason) VALUES($1,$2,$3,'cleanup','{}',$4,'Controlled historical NULL-locator note')")
                        .bind(Uuid::now_v7()).bind(actor).bind(Uuid::now_v7())
                        .bind(json!({"recordId":id,"kind":"segment","note":"Historical captured note"}))
                        .execute(&mut *tx).await.unwrap();
                    if n < 512 {
                        for revision in [2, 1] {
                            sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,verb,before_value,after_value,reason) VALUES($1,$2,$3,$4,$5,'time.correct','{}',$6,'Controlled ordered correction overlay')")
                                .bind(Uuid::now_v7()).bind(actor).bind(Uuid::now_v7()).bind(workspace).bind(task_id)
                                .bind(json!({"recordId":id,"kind":"segment","revision":revision,
                                    "startedAt":"2026-09-30T08:00:00Z","endedAt":if revision==2 {"2026-09-30T08:00:00.500Z"} else {"2026-09-30T08:00:00.250Z"},
                                    "note":if revision==2 {"Latest revision wins"} else {"Later UUID lower revision loses"}}))
                                .execute(&mut *tx).await.unwrap();
                        }
                    }
                }
                tx.commit().await.unwrap();
            }
            for table in [
                "time_entries",
                "task_timer_audit",
                "task_timer_runs",
                "task_timer_segments",
                "task_timer_legacy_open",
            ] {
                sqlx::query(&format!("ANALYZE fvoci.{table}"))
                    .execute(&admin)
                    .await
                    .unwrap();
            }
            for (name, mode) in [
                ("explain", Some(false)),
                ("first", Some(true)),
                ("warm", Some(true)),
                ("limited", None),
            ] {
                let observation =
                    measure_snapshot_statement(&fixture, workspace, task_id, actor, mode).await;
                timer_measurement_receipt(
                    &format!("snapshot-plan-{stage}-{name}.json"),
                    &json!({"stage":stage,"statement":"EXACT_PRODUCTION_FINGERPRINT",
                        "actualBackend":fixture.pid,"observation":observation}),
                );
                if mode.is_none() {
                    assert_eq!(observation["ids"].as_array().unwrap().len(), 101);
                    let allowed:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM fvoci.time_entries WHERE user_id=$1 AND workspace_id=$2 AND task_id=$3")
                        .bind(actor).bind(workspace).bind(task_id).fetch_all(&admin).await.unwrap();
                    assert!(observation["ids"].as_array().unwrap().iter().all(
                        |id| allowed.contains(&Uuid::parse_str(id.as_str().unwrap()).unwrap())
                    ));
                    if stage == "2048" {
                        original_fingerprint = Some(observation["fingerprint"].clone());
                    }
                    if stage == "mixed3072" {
                        assert_ne!(
                            original_fingerprint.as_ref().unwrap(),
                            &observation["fingerprint"]
                        );
                    }
                } else if mode == Some(true) {
                    println!(
                        "W5 SNAPSHOT PLAN stage={stage} order={name} executionMs={}",
                        observation[0]["Execution Time"]
                    );
                }
            }
            let started = std::time::Instant::now();
            let (status, page) =
                timer_checked_request(&fixture, app.clone(), "GET", &path, None, Some(&cookie))
                    .await;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            timer_measurement_receipt(
                &format!("snapshot-endpoint-{stage}.json"),
                &json!({"stage":stage,"status":status.as_u16(),"items":page["items"].as_array().map(Vec::len),"elapsedMs":elapsed}),
            );
            assert_eq!(status, StatusCode::OK, "{page}");
            assert_eq!(page["items"].as_array().unwrap().len(), 100);
            if stage == "mixed3072" {
                let (status, total) = timer_checked_request(
                    &fixture,
                    app.clone(),
                    "GET",
                    &format!("{base}/summary?from=2026-09-30&to=2026-09-30"),
                    None,
                    Some(&cookie),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{total}");
                assert_eq!(total["totalMilliseconds"],2048_i64*30000+512*500+512*750,
                    "all lower segments participate; maximum revision beats later lower-revision UUID");
            }
        }
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }
    #[tokio::test]
    async fn ordinary_time_entries_capture_preserves_private_projection_and_live_acl() {
        let harness = TestDb::bootstrap().await;
        let (fixture, app, cookie, actor, workspace) = TimerFixture::setup(&harness).await;
        let admin = admin_pool(&harness).await;
        let other = add_workspace_user(&admin, workspace, "member", "ordinary-capture").await;
        let project = create_project(app.clone(), &cookie, workspace, "READCTX", "workspace").await;
        let task = create_task(
            app.clone(),
            &cookie,
            workspace,
            project["id"].as_str().unwrap(),
            json!({"title":"Captured ordinary history"}),
        )
        .await;
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let base = format!("/api/v1/workspaces/{workspace}/tasks/{task_id}");
        let url = format!("{base}/time-entries");
        let intent = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-09-30T10:00:00Z","endedAt":"2026-09-30T10:15:00Z","note":"Shared original","reason":"Explicit manual interval"})).await;
        let (status, created) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!("{base}/timer/history"),
            Some(intent.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let record = &created["record"];
        let body = captured(app.clone(), &cookie, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":0,"expectedStartedAt":record["startedAt"],"expectedEndedAt":record["endedAt"],"expectedNote":record["note"],"startedAt":record["startedAt"],"endedAt":record["endedAt"],"note":"Author private correction","reason":"Private correction audit"})).await;
        let (status, corrected) = timer_checked_request(
            &fixture,
            app.clone(),
            "POST",
            &format!(
                "{base}/timer/records/{}/correct",
                record["id"].as_str().unwrap()
            ),
            Some(body),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{corrected}");
        let actors = [actor, other.user_id];
        let tasks = [task_id];
        let before = timer_effects(&admin, &actors, &tasks).await;
        let expected_actor = intent["expectedActorId"].as_str().unwrap();
        let expected_session = intent["expectedSessionId"].as_str().unwrap();
        // Planned future time is not elapsed history. Use the actual server
        // anchor and prove refusal preserves every product row and receipt.
        let (status, anchor) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &format!("{base}/timer"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{anchor}");
        let future_end =
            chrono::DateTime::parse_from_rfc3339(anchor["serverNow"].as_str().unwrap()).unwrap()
                + chrono::Duration::minutes(10);
        let mut future = intent.clone();
        future["requestId"] = json!(Uuid::now_v7());
        future["startedAt"] = json!((future_end - chrono::Duration::minutes(1)).to_rfc3339());
        future["endedAt"] = json!(future_end.to_rfc3339());
        let mut reasonless = intent.clone();
        reasonless["requestId"] = json!(Uuid::now_v7());
        reasonless["reason"] = json!("  ");
        for rejected in [future, reasonless] {
            let (status, invalid) = timer_checked_request(
                &fixture,
                app.clone(),
                "POST",
                &format!("{base}/timer/history"),
                Some(rejected),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}");
            assert!(invalid.get("record").is_none());
            assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        }
        let matched =
            format!("{url}?expectedActorId={expected_actor}&expectedSessionId={expected_session}");
        let mut own = None;
        // Both omitted and either matching partial capture retain the existing
        // timer guard policy. The actual browser always supplies both.
        for path in [
            url.clone(),
            matched.clone(),
            format!("{url}?expectedActorId={expected_actor}"),
            format!("{url}?expectedSessionId={expected_session}"),
        ] {
            let (status, result) =
                timer_checked_request(&fixture, app.clone(), "GET", &path, None, Some(&cookie))
                    .await;
            assert_eq!(status, StatusCode::OK, "{result}");
            assert_eq!(result["items"][0]["note"], "Author private correction");
            assert!(result.get("reason").is_none());
            if let Some(ref original) = own {
                assert_eq!(&result, original);
            } else {
                own = Some(result);
            }
        }
        // A PAT's captured credential is its actual token UUID, not a
        // cookie session. Keep the authenticated author's same private view.
        let read_token = api_token(&admin, actor, workspace, &["tasks.read"]).await;
        let pat_id: Uuid =
            sqlx::query_scalar("SELECT id FROM fvoci.api_tokens WHERE token_hash=$1")
                .bind(fvoci_server::auth::token::hash_token(&read_token))
                .fetch_one(&admin)
                .await
                .unwrap();
        let pat_path = format!("{url}?expectedActorId={actor}&expectedSessionId={pat_id}");
        for path in [&url, &pat_path] {
            fixture.probe("before captured PAT").await;
            let (status, rows) = bearer_request(app.clone(), "GET", path, None, &read_token).await;
            fixture.probe("after captured PAT").await;
            assert_eq!(status, StatusCode::OK, "{rows}");
            assert_eq!(Some(&rows), own.as_ref());
            assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        }
        for path in [
            format!(
                "{url}?expectedActorId={}&expectedSessionId={pat_id}",
                other.user_id
            ),
            format!(
                "{url}?expectedActorId={actor}&expectedSessionId={}",
                Uuid::now_v7()
            ),
            format!("{url}?expectedActorId={actor}&expectedSessionId={expected_session}"),
        ] {
            let (status, denied) =
                bearer_request(app.clone(), "GET", &path, None, &read_token).await;
            assert_eq!(status, StatusCode::CONFLICT, "{denied}");
            assert_eq!(denied["params"]["code"], "timer_context_changed");
            assert!(denied.get("items").is_none());
            assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        }
        let unrelated_scope = api_token(&admin, actor, workspace, &["projects.read"]).await;
        let unrelated_id: Uuid =
            sqlx::query_scalar("SELECT id FROM fvoci.api_tokens WHERE token_hash=$1")
                .bind(fvoci_server::auth::token::hash_token(&unrelated_scope))
                .fetch_one(&admin)
                .await
                .unwrap();
        let unrelated_path =
            format!("{url}?expectedActorId={actor}&expectedSessionId={unrelated_id}");
        let (status, denied) =
            bearer_request(app.clone(), "GET", &unrelated_path, None, &unrelated_scope).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
        assert!(denied.get("items").is_none());
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        // Existing cookie-first precedence is unchanged, even with a Bearer
        // that cannot read tasks. Capturing the PAT cannot override the cookie.
        let authorization = format!("Bearer {unrelated_scope}");
        let (status, rows, _) = http_request(
            app.clone(),
            "GET",
            &matched,
            None,
            Some("application/json"),
            Some(&cookie),
            &[("authorization", authorization.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{rows}");
        assert_eq!(Some(&rows), own.as_ref());
        let authorization = format!("Bearer {read_token}");
        let (status, denied, _) = http_request(
            app.clone(),
            "GET",
            &pat_path,
            None,
            Some("application/json"),
            Some(&cookie),
            &[("authorization", authorization.as_str())],
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{denied}");
        assert_eq!(denied["params"]["code"], "timer_context_changed");
        assert!(denied.get("items").is_none());
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        let (status, shared) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &url,
            None,
            Some(&other.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{shared}");
        assert_eq!(shared["items"][0]["note"], "Shared original");
        for (path, transport) in [
            (matched.clone(), other.cookie.as_str()),
            (
                format!("{url}?expectedActorId={}", other.user_id),
                cookie.as_str(),
            ),
            (
                format!("{url}?expectedSessionId={}", Uuid::now_v7()),
                cookie.as_str(),
            ),
        ] {
            let (status, denied) =
                timer_checked_request(&fixture, app.clone(), "GET", &path, None, Some(transport))
                    .await;
            assert_eq!(status, StatusCode::CONFLICT, "{denied}");
            assert_eq!(denied["params"]["code"], "timer_context_changed");
            assert!(denied.get("items").is_none());
            assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        }
        for query in [
            "expectedActorId=not-a-uuid",
            "expectedSessionId=",
            "unexpected=field",
        ] {
            let (status, invalid) = timer_checked_request(
                &fixture,
                app.clone(),
                "GET",
                &format!("{url}?{query}"),
                None,
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}");
            assert!(invalid.get("items").is_none());
        }
        let (status, login, headers) = project_harness::json_request_with_headers(
            app.clone(),
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email":"owner@example.com","password":"supersecret1"})),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{login}");
        let fresh = timer_session_cookie(&headers);
        let (status, old_capture) =
            timer_checked_request(&fixture, app.clone(), "GET", &matched, None, Some(&fresh)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{old_capture}");
        assert_eq!(old_capture["params"]["code"], "timer_context_changed");
        let fresh_context = captured(app.clone(), &fresh, json!({})).await;
        let fresh_path = format!(
            "{url}?expectedActorId={expected_actor}&expectedSessionId={}",
            fresh_context["expectedSessionId"].as_str().unwrap()
        );
        let (status, fresh_rows) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &fresh_path,
            None,
            Some(&fresh),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{fresh_rows}");
        assert_eq!(Some(fresh_rows), own);
        // Cross-tenant fixture preparation uses existing harness helpers.
        // Ordinary workspace members cannot create instance workspaces.
        let foreign_id = insert_workspace(&admin).await;
        let foreign_owner =
            add_workspace_user(&admin, foreign_id, "owner", "ordinary-foreign").await;
        let foreign_project = create_project(
            app.clone(),
            &foreign_owner.cookie,
            foreign_id,
            "FOREIGN",
            "workspace",
        )
        .await;
        let foreign_task = create_task(
            app.clone(),
            &foreign_owner.cookie,
            foreign_id,
            foreign_project["id"].as_str().unwrap(),
            json!({"title":"Other tenant target"}),
        )
        .await;
        let foreign_path = format!("/api/v1/workspaces/{foreign_id}/tasks/{}/time-entries?expectedActorId={expected_actor}&expectedSessionId={}",foreign_task["id"].as_str().unwrap(),fresh_context["expectedSessionId"].as_str().unwrap());
        let foreign_pat_path = format!("/api/v1/workspaces/{foreign_id}/tasks/{}/time-entries?expectedActorId={actor}&expectedSessionId={pat_id}",foreign_task["id"].as_str().unwrap());
        let (status, hidden_pat) =
            bearer_request(app.clone(), "GET", &foreign_pat_path, None, &read_token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{hidden_pat}");
        assert!(hidden_pat.get("items").is_none());
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        let (status, hidden) = timer_checked_request(
            &fixture,
            app.clone(),
            "GET",
            &foreign_path,
            None,
            Some(&fresh),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
        assert!(hidden.get("items").is_none());
        assert_eq!(timer_effects(&admin, &actors, &tasks).await, before);
        admin.close().await;
        drop(app);
        fixture.close().await;
        harness.cleanup().await;
    }
}
