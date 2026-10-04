//! W2 x W5 current schema: the estimate's declared unit travels with the
//! estimate on COPY and MOVE; a MOVE relocates the task's time graph (034
//! entries, the actor's 048 runs, segments and reservations) with the same
//! IDs while command receipts and audit rows stay untouched, and refuses
//! timer conflicts before effects; a COPY leaves the graph with the private
//! original. Timer state is created through the real W5 routes; only states
//! no current route can produce (a pre-048 open entry, another author's
//! entry) are labelled admin fixtures. The relocation fixtures need migration
//! 053 and the W5 replay/correction lookup (W8 union) to pass.

use super::*;
use chrono::Timelike;

/// The task's estimate and unit as committed (observer).
async fn estimate_of(
    run: &TestRun,
    workspace: Uuid,
    task: Uuid,
) -> (Option<String>, Option<String>) {
    let admin = admin_pool(&run.harness).await;
    let row: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT estimate::text, estimate_unit FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(task)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    row
}

/// Fixture (W5 minutes route not in this tree): an explicit minutes estimate.
async fn set_estimate(run: &TestRun, task: Uuid, unit: Option<&str>) {
    let admin = admin_pool(&run.harness).await;
    sqlx::query("UPDATE fvoci.tasks SET estimate = 90, estimate_unit = $2 WHERE id = $1")
        .bind(task)
        .bind(unit)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn personal_transfer_estimate_unit_travels_with_the_estimate_on_copy_and_move() {
    run_test(
        "personal_transfer_estimate_unit_travels_with_the_estimate_on_copy_and_move",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
            let team = actor.workspace_id;
            let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
            set_estimate(&run, task, Some("minutes")).await;
            assert_eq!(
                estimate_of(&run, source, task).await,
                (Some("90".into()), Some("minutes".into()))
            );
            // COPY: the new task carries the same pair; the original keeps its own.
            selection_versions(&run, source, &mut selection).await;
            let mut copy = selection.clone();
            copy["action"] = json!("copy");
            let body = command(addr, &actor, source, &copy).await;
            let (status, copied) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{copied}");
            let copied_task = Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
            assert_eq!(
                estimate_of(&run, team, copied_task).await,
                (Some("90".into()), Some("minutes".into()))
            );
            assert_eq!(
                estimate_of(&run, source, task).await,
                (Some("90".into()), Some("minutes".into()))
            );
            // A unit change after the review makes the MOVE preview stale.
            selection_versions(&run, source, &mut selection).await;
            let body = command(addr, &actor, source, &selection).await;
            set_estimate(&run, task, None).await;
            let (source_before, team_before) = (
                transfer_graph(&run, source).await,
                transfer_graph(&run, team).await,
            );
            let (status, error) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["params"]["code"], "preview_stale");
            assert_eq!(transfer_graph(&run, source).await, source_before);
            assert_eq!(transfer_graph(&run, team).await, team_before);
            // MOVE: same task ID, the same pair (here the explicit unit again).
            set_estimate(&run, task, Some("minutes")).await;
            let body = command(addr, &actor, source, &selection).await;
            let (status, moved) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{moved}");
            assert_eq!(moved["taskId"], task.to_string());
            assert_eq!(
                estimate_of(&run, team, task).await,
                (Some("90".into()), Some("minutes".into()))
            );
            run.finish().await.unwrap();
        },
    )
    .await;
}

/// The task's whole time graph (observer, admin): 034 entries, the actor's 048
/// runs/segments and reservations, without their tenant locator, plus every
/// workspace locator seen, and the actor's immutable command receipts and
/// audit rows in full.
async fn time_graph(run: &TestRun, task: Uuid, actor: Uuid) -> (Value, Value, Value) {
    let admin = admin_pool(&run.harness).await;
    let graph: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'entries',(SELECT coalesce(jsonb_agg(to_jsonb(e)-'workspace_id' ORDER BY e.id),'[]') FROM fvoci.time_entries e WHERE task_id=$1),
          'runs',(SELECT coalesce(jsonb_agg(to_jsonb(r)-'workspace_id' ORDER BY r.id),'[]') FROM fvoci.task_timer_runs r WHERE task_id=$1),
          'segments',(SELECT coalesce(jsonb_agg(to_jsonb(g)-'workspace_id' ORDER BY g.id),'[]') FROM fvoci.task_timer_segments g WHERE task_id=$1),
          'reserved',(SELECT coalesce(jsonb_agg(l.time_entry_id ORDER BY l.time_entry_id),'[]') FROM fvoci.task_timer_legacy_open l WHERE task_id=$1))",
    )
    .bind(task)
    .fetch_one(&admin)
    .await
    .unwrap();
    let tenants: Value = sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(DISTINCT w),'[]') FROM (
          SELECT workspace_id w FROM fvoci.time_entries WHERE task_id=$1 UNION ALL
          SELECT workspace_id FROM fvoci.task_timer_runs WHERE task_id=$1 UNION ALL
          SELECT workspace_id FROM fvoci.task_timer_segments WHERE task_id=$1 UNION ALL
          SELECT workspace_id FROM fvoci.task_timer_legacy_open WHERE task_id=$1) t",
    )
    .bind(task)
    .fetch_one(&admin)
    .await
    .unwrap();
    let history: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
          'commands',(SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.request_id),'[]') FROM fvoci.task_timer_commands c WHERE user_id=$1),
          'audit',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]') FROM fvoci.task_timer_audit a WHERE user_id=$1))",
    )
    .bind(actor)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    (graph, tenants, history)
}

/// The MOVE preview refuses with the typed code and title, and neither the
/// time graph, the immutable receipts/audit nor either transfer graph change.
#[allow(clippy::too_many_arguments)]
async fn refuse_timer_preview(
    addr: SocketAddr,
    run: &TestRun,
    actor: &SessionFixture,
    source: Uuid,
    task: Uuid,
    selection: &Value,
    code: &str,
    title: &str,
) {
    let team = actor.workspace_id;
    let before = time_graph(run, task, actor.user_id).await;
    let (source_before, team_before) = (
        transfer_graph(run, source).await,
        transfer_graph(run, team).await,
    );
    let (status, error) = session_call(
        addr,
        Method::POST,
        &format!("{}/preview", path(source)),
        &actor.session_token,
        Some(selection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(
        (error["params"]["code"].as_str(), error["title"].as_str()),
        (Some(code), Some(title)),
        "{error}"
    );
    assert_eq!(time_graph(run, task, actor.user_id).await, before);
    assert_eq!(transfer_graph(run, source).await, source_before);
    assert_eq!(transfer_graph(run, team).await, team_before);
}

/// A W5 timer request body with the caller's captured identity.
fn timer_body(actor: &SessionFixture, mut body: Value) -> Value {
    body["expectedActorId"] = json!(actor.user_id);
    body["expectedSessionId"] = json!(actor.session_id);
    body
}

/// Starts (or stops) the owner's stopwatch through the W5 route.
async fn timer_op(
    addr: SocketAddr,
    actor: &SessionFixture,
    base: &str,
    operation: &str,
    version: Value,
    run: Value,
) -> Value {
    let body = timer_body(
        actor,
        json!({"requestId":Uuid::now_v7(),"operation":operation,"expectedVersion":version,"runId":run,"note":"자료 읽기"}),
    );
    let (status, out) = session_call(
        addr,
        Method::POST,
        &format!("{base}/timer"),
        &actor.session_token,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{operation}: {out}");
    out
}

#[tokio::test]
async fn personal_transfer_move_relocates_the_time_graph_with_the_same_ids() {
    run_test("personal_transfer_move_relocates_the_time_graph_with_the_same_ids", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let base = |ws: Uuid| format!("/api/v1/workspaces/{ws}/tasks/{task}");
        // Real W5 routes: a stopped run (segment + its 034 projection), a
        // manual entry corrected once, and a running run.
        let first = timer_op(addr, &actor, &base(source), "start", json!(0), Value::Null).await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        timer_op(addr, &actor, &base(source), "stop", first["version"].clone(), first["runId"].clone()).await;
        // Times relative to now (the history range is under 32 days), whole
        // seconds, in the API's millisecond form.
        let at = |minutes: i64| {
            (Utc::now() - ChronoDuration::minutes(minutes))
                .with_nanosecond(0)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        };
        let manual = timer_body(&actor, json!({"requestId":Uuid::now_v7(),"startedAt":at(180),"endedAt":at(150),"note":"수동 기록","reason":"기록 추가"}));
        let (status, created) = session_call(addr, Method::POST, &format!("{}/timer/history", base(source)), &actor.session_token, Some(manual)).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let record = created["record"].clone();
        let corrected_start = at(120);
        let correction = timer_body(&actor, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":record["revision"],"expectedNote":record["note"],
            "expectedStartedAt":record["startedAt"],"expectedEndedAt":record["endedAt"],"startedAt":corrected_start,"endedAt":at(75),"note":"고친 기록","reason":"시간 수정"}));
        let (status, corrected) = session_call(addr, Method::POST, &format!("{}/timer/records/{}/correct", base(source), record["id"].as_str().unwrap()), &actor.session_token, Some(correction)).await;
        assert_eq!(status, StatusCode::OK, "{corrected}");
        let running = timer_op(addr, &actor, &base(source), "start", json!(0), Value::Null).await;
        let (before, tenants, history) = time_graph(&run, task, actor.user_id).await;
        assert_eq!(tenants, json!([source]));
        assert_eq!(before["entries"].as_array().unwrap().len(), 2, "{before}");
        assert_eq!(before["runs"].as_array().unwrap().len(), 2, "{before}");
        // The preview discloses the time graph that moves.
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        for (item, count) in [("time_entry", 2), ("timer", 2)] {
            assert!(preview["dispositions"].as_array().unwrap().iter().any(|d| d["item"] == item && d["outcome"] == "moved" && d["count"] == count), "{item}: {preview}");
        }
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        // Same IDs and every value; only the tenant locator changed; the
        // command receipts and audit rows are untouched.
        let (after, tenants, history_after) = time_graph(&run, task, actor.user_id).await;
        assert_eq!(after, before);
        assert_eq!(tenants, json!([team]));
        assert_eq!(history_after, history, "immutable receipts and audit");
        // The owner reads the moved graph at the destination: the correction
        // still applies to its record, and the running run is the same one.
        let (status, listed) = session_call(addr, Method::GET, &format!("{}/timer/history?expectedActorId={}&expectedSessionId={}&from={}&to={}", base(team), actor.user_id, actor.session_id, (Utc::now() - ChronoDuration::days(1)).date_naive(), (Utc::now() + ChronoDuration::days(1)).date_naive()), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let item = listed["items"].as_array().unwrap().iter().find(|i| i["id"] == record["id"]).unwrap_or_else(|| panic!("{listed}"));
        // The same instant (the API may serialize whole seconds without
        // milliseconds), and the corrected note.
        let instant = |value: &str| chrono::DateTime::parse_from_rfc3339(value).unwrap_or_else(|_| panic!("{value}"));
        assert_eq!(instant(item["startedAt"].as_str().unwrap()), instant(&corrected_start), "{item}");
        assert_eq!(item["note"], json!("고친 기록"), "{item}");
        timer_op(addr, &actor, &base(team), "stop", running["version"].clone(), running["runId"].clone()).await;
        // The private source no longer serves the task's timer.
        let (status, _) = session_call(addr, Method::GET, &format!("{}/timer", base(source)), &actor.session_token, None).await;
        assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "{status}");
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_move_refuses_timer_conflicts_and_keeps_a_released_open_entry_released() {
    run_test("personal_transfer_move_refuses_timer_conflicts_and_keeps_a_released_open_entry_released", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let admin = admin_pool(&run.harness).await;
        // Fixture: a pre-048 legacy open entry (its trigger creates the
        // reservation), then released through the W5 route.
        let open = Uuid::now_v7();
        let mut fixture = admin.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.self_user_id',$1,true)").bind(actor.user_id.to_string()).execute(&mut *fixture).await.unwrap();
        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,note) VALUES($1,$2,$3,$4,now()-interval '1 hour','열린 기록')")
            .bind(open).bind(source).bind(task).bind(actor.user_id).execute(&mut *fixture).await.unwrap();
        fixture.commit().await.unwrap();
        let release = timer_body(&actor, json!({"requestId":Uuid::now_v7(),"timeEntryId":open}));
        let (status, released) = session_call(addr, Method::POST, "/api/v1/me/task-timer/legacy-release", &actor.session_token, Some(release)).await;
        assert_eq!(status, StatusCode::OK, "{released}");
        let (released_graph, _, _) = time_graph(&run, task, actor.user_id).await;
        assert_eq!(released_graph["reserved"], json!([]));
        selection_versions(&run, source, &mut selection).await;
        // An unfinished run of the actor on another task: the open entry could
        // not be reinserted, so the MOVE refuses before effects.
        let other = create_task(addr, &actor, project).await;
        let elsewhere = format!("/api/v1/workspaces/{team}/tasks/{other}");
        let busy = timer_op(addr, &actor, &elsewhere, "start", json!(0), Value::Null).await;
        selection_versions(&run, source, &mut selection).await;
        refuse_timer_preview(addr, &run, &actor, source, task, &selection, "timer_busy", "another unfinished timer or open time entry").await;
        timer_op(addr, &actor, &elsewhere, "stop", busy["version"].clone(), busy["runId"].clone()).await;
        // An open entry of the actor already in the destination (034 allows one).
        let blocking = Uuid::now_v7();
        let mut fixture = admin.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.self_user_id',$1,true)").bind(actor.user_id.to_string()).execute(&mut *fixture).await.unwrap();
        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at) VALUES($1,$2,$3,$4,now()-interval '2 hours')")
            .bind(blocking).bind(team).bind(other).bind(actor.user_id).execute(&mut *fixture).await.unwrap();
        sqlx::query("DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id=$1").bind(blocking).execute(&mut *fixture).await.unwrap();
        fixture.commit().await.unwrap();
        selection_versions(&run, source, &mut selection).await;
        refuse_timer_preview(addr, &run, &actor, source, task, &selection, "timer_busy", "another unfinished timer or open time entry").await;
        sqlx::query("UPDATE fvoci.time_entries SET ended_at=started_at+interval '1 minute', duration_seconds=60 WHERE id=$1").bind(blocking).execute(&admin).await.unwrap();
        // A time entry by another author (not exportable as the owner's).
        let stranger = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.users(id,email,given_name) VALUES($1,$2,'다른 사람')").bind(stranger).bind(format!("{stranger}@example.com")).execute(&admin).await.unwrap();
        let foreign = Uuid::now_v7();
        sqlx::query("INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds) VALUES($1,$2,$3,$4,now()-interval '3 hours',now()-interval '170 minutes',600)")
            .bind(foreign).bind(source).bind(task).bind(stranger).execute(&admin).await.unwrap();
        selection_versions(&run, source, &mut selection).await;
        refuse_timer_preview(addr, &run, &actor, source, task, &selection, "dependent_graph", "time entry by another author").await;
        sqlx::query("DELETE FROM fvoci.time_entries WHERE id=$1").bind(foreign).execute(&admin).await.unwrap();
        admin.close().await;
        // Now it moves: the open entry stays open and stays released.
        selection_versions(&run, source, &mut selection).await;
        let (before, _, history) = time_graph(&run, task, actor.user_id).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        let (after, tenants, history_after) = time_graph(&run, task, actor.user_id).await;
        assert_eq!(after, before);
        assert_eq!(after["reserved"], json!([]), "released stays released");
        assert_eq!(tenants, json!([team]));
        assert_eq!(history_after, history);
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_leaves_the_time_graph_with_the_private_original() {
    run_test("personal_transfer_copy_leaves_the_time_graph_with_the_private_original", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let base = format!("/api/v1/workspaces/{source}/tasks/{task}");
        let first = timer_op(addr, &actor, &base, "start", json!(0), Value::Null).await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        timer_op(addr, &actor, &base, "stop", first["version"].clone(), first["runId"].clone()).await;
        timer_op(addr, &actor, &base, "start", json!(0), Value::Null).await;
        let (before, tenants, history) = time_graph(&run, task, actor.user_id).await;
        assert_eq!(tenants, json!([source]));
        selection_versions(&run, source, &mut selection).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(copy.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        for (item, count) in [("time_entry", 1), ("timer", 2)] {
            assert!(preview["dispositions"].as_array().unwrap().iter().any(|d| d["item"] == item && d["outcome"] == "retained_private" && d["count"] == count), "{item}: {preview}");
        }
        let body = json!({"requestId":Uuid::now_v7(),"selection":copy,"previewDigest":preview["digest"],"confirmed":true});
        let (status, copied) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{copied}");
        let copied_task = Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
        let (copy_graph, copy_tenants, _) = time_graph(&run, copied_task, actor.user_id).await;
        assert_eq!(copy_graph, json!({"entries":[],"runs":[],"segments":[],"reserved":[]}));
        assert_eq!(copy_tenants, json!([]));
        assert_eq!(time_graph(&run, task, actor.user_id).await, (before, tenants, history));
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn timer_commands_and_audit_are_append_only_for_the_app_role() {
    run_test("timer_commands_and_audit_are_append_only_for_the_app_role", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (_, actor, _, _, _, _) = setup_transfer(&mut run).await;
        let (request, audit) = (Uuid::now_v7(), Uuid::now_v7());
        // The restricted app role, in its own actor context: append and read.
        let mut tx = actor.pool.begin().await.unwrap();
        fvoci_server::db::context::set_self_user(&mut tx, actor.user_id).await.unwrap();
        sqlx::query("INSERT INTO fvoci.task_timer_commands(user_id,request_id,request_hash,result) VALUES($1,$2,repeat('a',64),'{\"ok\":true}')")
            .bind(actor.user_id)
            .bind(request)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,verb,before_value,after_value,reason) VALUES($1,$2,$3,'task.estimate.minutes','{}','{}','fixture')")
            .bind(audit)
            .bind(actor.user_id)
            .bind(request)
            .execute(&mut *tx)
            .await
            .unwrap();
        let seen: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.task_timer_commands WHERE request_id=$1), (SELECT count(*) FROM fvoci.task_timer_audit WHERE id=$2)")
            .bind(request)
            .bind(audit)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(seen, (1, 1));
        tx.commit().await.unwrap();
        // UPDATE and DELETE are refused by privilege, each in its own transaction.
        for sql in [
            "UPDATE fvoci.task_timer_commands SET result='{}' WHERE request_id=$1",
            "DELETE FROM fvoci.task_timer_commands WHERE request_id=$1",
            "UPDATE fvoci.task_timer_audit SET reason='changed' WHERE request_id=$1",
            "DELETE FROM fvoci.task_timer_audit WHERE request_id=$1",
        ] {
            let mut tx = actor.pool.begin().await.unwrap();
            fvoci_server::db::context::set_self_user(&mut tx, actor.user_id).await.unwrap();
            let error = sqlx::query(sql).bind(request).execute(&mut *tx).await.expect_err(sql);
            let code = error.as_database_error().and_then(|e| e.code()).map(|c| c.to_string());
            assert_eq!(code.as_deref(), Some("42501"), "{sql}: {error}");
            tx.rollback().await.unwrap();
        }
        let admin = admin_pool(&run.harness).await;
        let kept: (Value, String) = sqlx::query_as("SELECT (SELECT result FROM fvoci.task_timer_commands WHERE request_id=$1), (SELECT reason FROM fvoci.task_timer_audit WHERE id=$2)")
            .bind(request)
            .bind(audit)
            .fetch_one(&admin)
            .await
            .unwrap();
        assert_eq!(kept, (json!({"ok": true}), "fixture".to_string()));
        admin.close().await;
        run.finish().await.unwrap();
    })
    .await;
}
