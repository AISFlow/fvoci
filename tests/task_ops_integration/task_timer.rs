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
}
