//! W2 x W5 current schema (048/049, adopted byte-exact in 37bcc96): the
//! estimate's declared unit travels with the estimate on COPY and MOVE, and
//! a MOVE refuses before effects while the owner's private timer rows exist
//! (the task delete would cascade them away). The W5 minutes and timer
//! routes are not in this tree yet, so the rows are written by a labelled
//! admin fixture with the current schema's own constraints.

use super::*;

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

/// The owner's timer rows of a task (observer).
async fn timer_rows(run: &TestRun, task: Uuid) -> Value {
    let admin = admin_pool(&run.harness).await;
    let rows: Value = sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]'::jsonb) FROM fvoci.task_timer_runs r WHERE task_id=$1",
    )
    .bind(task)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    rows
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

#[tokio::test]
async fn personal_transfer_move_refuses_owner_timer_rows_and_copy_leaves_them() {
    run_test("personal_transfer_move_refuses_owner_timer_rows_and_copy_leaves_them", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let team = actor.workspace_id;
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        // Fixture (W5 timer route not in this tree): the owner's running timer.
        let run_id = Uuid::now_v7();
        let admin = admin_pool(&run.harness).await;
        sqlx::query("INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at) VALUES($1,$2,$3,$4,'running',1,now())")
            .bind(run_id)
            .bind(actor.user_id)
            .bind(source)
            .bind(task)
            .execute(&admin)
            .await
            .unwrap();
        let timers = timer_rows(&run, task).await;
        assert_eq!(timers.as_array().unwrap().len(), 1);
        selection_versions(&run, source, &mut selection).await;
        for status_sql in [None, Some("UPDATE fvoci.task_timer_runs SET status='stopped', stopped_at=now(), version=2 WHERE id=$1")] {
            if let Some(sql) = status_sql {
                sqlx::query(sql).bind(run_id).execute(&admin).await.unwrap();
            }
            let timers = timer_rows(&run, task).await;
            let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, team).await);
            let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["params"]["code"], "dependent_graph");
            assert_eq!(transfer_graph(&run, source).await, source_before);
            assert_eq!(transfer_graph(&run, team).await, team_before);
            assert_eq!(timer_rows(&run, task).await, timers, "timer rows untouched");
        }
        admin.close().await;
        // COPY: a new task without timers; the original's timer stays as is.
        let timers = timer_rows(&run, task).await;
        let mut copy = selection.clone();
        copy["action"] = json!("copy");
        let body = command(addr, &actor, source, &copy).await;
        let (status, copied) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{copied}");
        let copied_task = Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
        assert_eq!(timer_rows(&run, copied_task).await, json!([]));
        assert_eq!(timer_rows(&run, task).await, timers);
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
