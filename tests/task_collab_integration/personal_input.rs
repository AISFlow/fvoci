//! New product requests; fixture admin only prepares triggers/users. Every
//! committed assertion uses a fresh NOSUPERUSER/NOBYPASSRLS app connection.
use super::*;
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection};

async fn observer(run: &TestRun, ws: Uuid) -> PgConnection {
    let mut connection = PgConnection::connect(&run.harness.app_url).await.unwrap();
    let flags: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(flags, (false, false));
    let forced:bool=sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class WHERE oid = 'fvoci.personal_input_commands'::regclass").fetch_one(&mut connection).await.unwrap();
    assert!(forced);
    sqlx::query("BEGIN").execute(&mut connection).await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id',$1,true)")
        .bind(ws.to_string())
        .execute(&mut connection)
        .await
        .unwrap();
    connection
}
async fn personal(addr: SocketAddr, owner: &SessionFixture) -> SessionFixture {
    // Shared collab fixture uses a personal-shaped slug for its ordinary team.
    // Give that fixture a distinct team slug; real users start with team slugs.
    let mut tx = owner.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, owner.workspace_id)
        .await
        .unwrap();
    let team_kind: String = sqlx::query_scalar("SELECT kind FROM fvoci.workspaces WHERE id=$1")
        .bind(owner.workspace_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(team_kind, "team");
    sqlx::query("UPDATE fvoci.workspaces SET slug=$2 WHERE id=$1 AND kind='team'")
        .bind(owner.workspace_id)
        .bind(format!(
            "team-{}",
            &owner.workspace_id.simple().to_string()[20..]
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, value) = session_call(
        addr,
        Method::POST,
        "/api/v1/me/personal-workspace",
        &owner.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        value["slug"],
        fvoci_server::db::workspace::personal_workspace_slug(owner.user_id)
    );
    let personal_id = Uuid::parse_str(value["id"].as_str().unwrap()).unwrap();
    assert_ne!(personal_id, owner.workspace_id);
    let mut tx = owner.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, personal_id)
        .await
        .unwrap();
    let actual: (String, Uuid, String) = sqlx::query_as("SELECT w.kind,u.personal_workspace_id,m.role FROM fvoci.workspaces w JOIN fvoci.users u ON u.personal_workspace_id=w.id JOIN fvoci.memberships m ON m.workspace_id=w.id AND m.user_id=u.id WHERE w.id=$1 AND u.id=$2")
        .bind(personal_id).bind(owner.user_id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(actual, ("personal".into(), personal_id, "owner".into()));
    tx.commit().await.unwrap();
    SessionFixture {
        workspace_id: Uuid::parse_str(value["id"].as_str().unwrap()).unwrap(),
        pool: owner.pool.clone(),
        user_id: owner.user_id,
        session_id: owner.session_id,
        session_token: owner.session_token.clone(),
    }
}
fn input_path(owner: &SessionFixture) -> String {
    format!("/api/v1/workspaces/{}/personal-input", owner.workspace_id)
}
async fn committed_counts(
    run: &TestRun,
    ws: Uuid,
    title: &str,
) -> (i64, i64, i64, i64, i64, i64, i64) {
    let mut conn = observer(run, ws).await;
    // Audit's actual FORCE RLS policy permits system context; keep restricted
    // role and explicit tenant in this fresh fixture observer transaction.
    sqlx::query("SET LOCAL app.system_ctx='on'")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query_as(r#"SELECT
        (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1 AND title=$2),
        (SELECT count(*) FROM fvoci.tasks WHERE workspace_id=$1 AND title=$2),
        (SELECT count(*) FROM fvoci.task_origins o JOIN fvoci.tasks t ON t.workspace_id=o.workspace_id AND t.id=o.task_id WHERE t.workspace_id=$1 AND t.title=$2),
        (SELECT count(*) FROM fvoci.task_activity a JOIN fvoci.tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE t.workspace_id=$1 AND t.title=$2),
        (SELECT count(*) FROM fvoci.events e WHERE e.workspace_id=$1 AND e.verb='task.created' AND e.payload->>'title'=$2),
        (SELECT count(*) FROM fvoci.audit_log a WHERE a.workspace_id=$1 AND a.verb='task.created' AND a.payload->>'title'=$2),
        (SELECT count(*) FROM fvoci.task_assignees a JOIN fvoci.tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE t.workspace_id=$1 AND t.title=$2)
    "#).bind(ws).bind(title).fetch_one(&mut conn).await.unwrap()
}

#[tokio::test]
async fn personal_input_committed_identity_retry_assignment_and_private_boundary() {
    run_test("personal_input_committed_identity_retry_assignment_and_private_boundary",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,f)=setup_task(&mut run,5000).await;
    let owner=personal(addr,&f.owner).await;let ws=owner.workspace_id;let path=input_path(&owner);
    let body=json!({"requestId":Uuid::now_v7(),"intent":"task","title":"개인 research 🙂"});
    let (a,b)=tokio::join!(session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())),session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())));
    assert_eq!(a.0,StatusCode::CREATED,"{}",a.1);assert_eq!(b.0,StatusCode::CREATED,"{}",b.1);
    assert_eq!(a.1["taskId"],b.1["taskId"]);assert_eq!(a.1["documentId"],b.1["documentId"]);
    let task=Uuid::parse_str(a.1["taskId"].as_str().unwrap()).unwrap();let document=Uuid::parse_str(a.1["documentId"].as_str().unwrap()).unwrap();
    assert_eq!(committed_counts(&run,ws,"개인 research 🙂").await,(1,1,1,1,1,1,1));
    let mut conn=observer(&run,ws).await;
    let row:(Uuid,Uuid,Uuid,Uuid)=sqlx::query_as("SELECT c.document_id,c.task_id,o.document_id,a.user_id FROM fvoci.personal_input_commands c JOIN fvoci.task_origins o ON o.workspace_id=c.workspace_id AND o.task_id=c.task_id JOIN fvoci.task_assignees a ON a.workspace_id=c.workspace_id AND a.task_id=c.task_id WHERE c.workspace_id=$1 AND c.request_id=$2")
        .bind(ws).bind(Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap()).fetch_one(&mut conn).await.unwrap();
    assert_eq!(row,(document,task,document,owner.user_id));drop(conn);
    let (status,list)=session_call(addr,Method::GET,&format!("/api/v1/workspaces/{ws}/tasks?query=%7B%22filters%22%3A%7B%22assigneeId%22%3A%22me%22%2C%22openOnly%22%3Atrue%7D%7D"),&owner.session_token,None).await;
    assert_eq!(status,StatusCode::OK,"{list}");assert!(list["items"].as_array().unwrap().iter().any(|row|row["id"]==task.to_string()));
    let mut changed=body.clone();changed["title"]=json!("changed");let(status,problem)=session_call(addr,Method::POST,&path,&owner.session_token,Some(changed)).await;
    assert_eq!(status,StatusCode::CONFLICT,"{problem}");assert_eq!(problem["code"],"document_version_mismatch");
    let mut whitespace = body.clone(); whitespace["title"] = json!(format!("{} ", body["title"].as_str().unwrap()));
    let (status, problem) = session_call(addr, Method::POST, &path, &owner.session_token, Some(whitespace)).await;
    assert_eq!(status, StatusCode::CONFLICT, "changed typed title must conflict: {problem}");
    assert_eq!(problem["code"], "document_version_mismatch");
    assert_eq!(committed_counts(&run, ws, "개인 research 🙂").await, (1,1,1,1,1,1,1));
    let (status,replay)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::CREATED);assert_eq!(replay["taskId"],task.to_string());
    // A team wiki is not private: the new API rejects this route even for owner.
    let(status,_)=session_call(addr,Method::POST,&input_path(&f.owner),&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::NOT_FOUND);
    let outsider=create_user_session(&run.harness,f.owner.workspace_id,WorkspaceRole::Member).await;
    for suffix in [format!("documents/{document}"),format!("tasks/{task}"),format!("tasks/{task}/origin"),format!("tasks/{task}/backlinks")] {
      let(status,value)=session_call(addr,Method::GET,&format!("/api/v1/workspaces/{ws}/{suffix}"),&outsider.session_token,None).await;
      assert_eq!(status,StatusCode::NOT_FOUND,"{value}");assert!(!value.to_string().contains("research"));
    }
    let denied=admission(addr,&outsider.session_token,&task_key(ws,task),91).await;assert!(denied.is_err());
    let project=Uuid::parse_str(a.1["projectId"].as_str().unwrap()).unwrap();
    let(status,stream)=session_call(addr,Method::GET,&format!("/api/v1/workspaces/{ws}/projects/{project}/stream"),&outsider.session_token,None).await;assert_eq!(status,StatusCode::NOT_FOUND,"{stream}");
    let(status,note)=session_call(addr,Method::POST,&path,&owner.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"note","title":"메모는 태스크가 아니다"}))).await;assert_eq!(status,StatusCode::CREATED,"{note}");assert!(note["taskId"].is_null());assert_eq!(committed_counts(&run,ws,"메모는 태스크가 아니다").await,(1,0,0,0,0,0,0));
    let(status,quick)=session_call(addr,Method::POST,&path,&owner.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"quick","title":"분류 없이 빠른 기록"}))).await;
    assert_eq!(status,StatusCode::CREATED,"{quick}"); assert!(quick["taskId"].is_null());
    assert_eq!(committed_counts(&run,ws,"분류 없이 빠른 기록").await,(1,0,0,0,0,0,0));
    // Ordinary team task creation remains unassigned.
    let(status,control)=session_call(addr,Method::GET,&task_path(&f.owner,f.task_id,""),&f.owner.session_token,None).await;assert_eq!(status,StatusCode::OK);assert_eq!(control["assigneeIds"],json!([]));
    let(status,logout)=session_call(addr,Method::POST,"/api/v1/auth/logout",&owner.session_token,None).await;
    assert_eq!(status,StatusCode::NO_CONTENT,"{logout}");
    let(status,_)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body)).await;
    assert_eq!(status,StatusCode::UNAUTHORIZED);
    let(status,_)=session_call(addr,Method::POST,"/api/v1/me/personal-workspace",&owner.session_token,None).await;
    assert_eq!(status,StatusCode::UNAUTHORIZED);
    assert_eq!(committed_counts(&run,ws,"개인 research 🙂").await,(1,1,1,1,1,1,1));
    run.finish().await.unwrap();
 }).await;
}

#[tokio::test]
async fn personal_input_soft_and_hard_deleted_outcome_never_recreates() {
    run_test("personal_input_soft_and_hard_deleted_outcome_never_recreates",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,f)=setup_task(&mut run,5000).await;let owner=personal(addr,&f.owner).await;let ws=owner.workspace_id;let path=input_path(&owner);
    let body=json!({"requestId":Uuid::now_v7(),"intent":"note","title":"retired outcome"});let(status,value)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::CREATED,"{value}");let doc=Uuid::parse_str(value["documentId"].as_str().unwrap()).unwrap();
    let(status,trash)=session_call(addr,Method::DELETE,&document_api(ws,doc,""),&owner.session_token,None).await;assert_eq!(status,StatusCode::OK,"{trash}");let(status,_)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::NOT_FOUND);
    // Fixture purge through restricted role. FK retires target only; receipt survives.
    let mut conn=observer(&run,ws).await;sqlx::query("DELETE FROM fvoci.documents WHERE workspace_id=$1 AND id=$2").bind(ws).bind(doc).execute(&mut conn).await.unwrap();sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    let(status,_)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::NOT_FOUND);
    let mut conn=observer(&run,ws).await;let receipt:(i64,i64)=sqlx::query_as("SELECT count(*),count(document_id) FROM fvoci.personal_input_commands WHERE workspace_id=$1 AND request_id=$2").bind(ws).bind(Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap()).fetch_one(&mut conn).await.unwrap();assert_eq!(receipt,(1,0));assert_eq!(committed_counts(&run,ws,"retired outcome").await,(0,0,0,0,0,0,0));drop(conn);run.finish().await.unwrap();
 }).await;
}

#[tokio::test]
async fn personal_input_origin_failure_rolls_back_task_events_activity_and_receipt() {
    run_test("personal_input_origin_failure_rolls_back_task_events_activity_and_receipt",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,f)=setup_task(&mut run,5000).await;let owner=personal(addr,&f.owner).await;let ws=owner.workspace_id;let path=input_path(&owner);let request=Uuid::now_v7();
    let admin=admin_pool(&run.harness).await;
    sqlx::raw_sql(&format!(r#"CREATE FUNCTION fvoci.fixture_origin_fail() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, fvoci AS $$ BEGIN
      IF NEW.request_id = '{request}'::uuid THEN
        IF EXISTS(SELECT 1 FROM fvoci.task_activity WHERE task_id=NEW.task_id)
           AND EXISTS(SELECT 1 FROM fvoci.events WHERE target_id=NEW.task_id AND verb='task.created')
           AND EXISTS(SELECT 1 FROM fvoci.audit_log WHERE target_id=NEW.task_id AND verb='task.created')
           AND EXISTS(SELECT 1 FROM fvoci.task_assignees WHERE task_id=NEW.task_id)
        THEN RAISE EXCEPTION 'fixture_after_task_event_activity_before_origin'; END IF;
      END IF; RETURN NEW; END $$;
      CREATE TRIGGER fixture_origin_fail BEFORE INSERT ON fvoci.task_origins FOR EACH ROW EXECUTE FUNCTION fvoci.fixture_origin_fail();"#)).execute(&admin).await.unwrap();
    let body=json!({"requestId":request,"intent":"task","title":"rollback fixture"});let(status,_)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;
    sqlx::raw_sql("DROP TRIGGER fixture_origin_fail ON fvoci.task_origins; DROP FUNCTION fvoci.fixture_origin_fail();").execute(&admin).await.unwrap();admin.close().await;
    assert_eq!(status,StatusCode::INTERNAL_SERVER_ERROR);assert_eq!(committed_counts(&run,ws,"rollback fixture").await,(0,0,0,0,0,0,0));
    assert_eq!(default_project_counts(&run, ws).await, (0,0,0,0,0), "default project, workflow, statuses, documents and receipt all roll back");
    let mut conn=observer(&run,ws).await;let receipts:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.personal_input_commands WHERE request_id=$1").bind(request).fetch_one(&mut conn).await.unwrap();assert_eq!(receipts,0);drop(conn);
    let(status,created)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body)).await;assert_eq!(status,StatusCode::CREATED,"{created}");assert_eq!(committed_counts(&run,ws,"rollback fixture").await,(1,1,1,1,1,1,1));run.finish().await.unwrap();
 }).await;
}

#[tokio::test]
async fn personal_input_legacy_stored_origin_hash_omitted_false_and_changed_true() {
    run_test("personal_input_legacy_stored_origin_hash_omitted_false_and_changed_true",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,f)=setup_task(&mut run,5000).await;let owner=personal(addr,&f.owner).await;let ws=owner.workspace_id;let project=create_project(addr,&owner,"LEGACY").await;let doc=create_wiki(&owner,"legacy source").await;let task=create_task(addr,&owner,project).await;let request=Uuid::now_v7();
    // Frozen legacy canonical byte template, authored independently of current
    // normalized_task_input/origin_request_hash (defaults were task/none).
    let canonical=format!(r#"{{"anchor":null,"projectId":"{project}","task":{{"dueDate":null,"milestoneId":null,"parentId":null,"priority":"none","recurrence":null,"startDate":null,"statusId":null,"title":"Legacy","type":"task"}},"userId":"{}"}}"#,owner.user_id);
    let legacy=hex::encode(Sha256::digest(canonical.as_bytes()));let mut conn=observer(&run,ws).await;
    sqlx::query("INSERT INTO fvoci.task_origins (workspace_id,task_id,document_id,request_id,request_hash) VALUES($1,$2,$3,$4,$5)").bind(ws).bind(task).bind(doc).bind(request).bind(&legacy).execute(&mut conn).await.unwrap();sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    let path=document_api(ws,doc,"/tasks");let mut body=json!({"projectId":project,"requestId":request,"task":{"title":"Legacy"}});
    for explicit in [None,Some(false)] {
      if let Some(value)=explicit {body["selfAssign"]=json!(value);}
      let(status,replay)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::CREATED,"{replay}");assert_eq!(replay["taskId"],task.to_string());
    }
    body["selfAssign"]=json!(true);let(status,problem)=session_call(addr,Method::POST,&path,&owner.session_token,Some(body)).await;assert_eq!(status,StatusCode::CONFLICT,"{problem}");
    run.finish().await.unwrap();
 }).await;
}

async fn blocked_by(conn: &mut PgConnection, blocker: i32) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        sqlx::query("SELECT pg_stat_clear_snapshot()")
            .execute(&mut *conn)
            .await
            .unwrap();
        let waiting:Option<i32>=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND $1=ANY(pg_blocking_pids(pid)) AND usename=current_user ORDER BY pid LIMIT 1").bind(blocker).fetch_optional(&mut *conn).await.unwrap();
        if let Some(pid) = waiting {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no product lock waiter observed for blocker {blocker}"
        );
        tokio::task::yield_now().await;
    }
}
async fn source_fixture(
    run: &mut TestRun,
) -> (SocketAddr, SessionFixture, SessionFixture, Uuid, Uuid, Uuid) {
    let (addr, f) = setup_task(run, 5000).await;
    let owner = f.owner;
    let target = f.project_id;
    let source = create_named_project(addr, &owner, "SOURCE", "workspace").await;
    assert!(
        target < source,
        "fixture must hold lower target before final source serialization"
    );
    let (status, meta) = session_call(
        addr,
        Method::GET,
        &format!(
            "/api/v1/workspaces/{}/projects/{source}",
            owner.workspace_id
        ),
        &owner.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    let document = Uuid::parse_str(meta["rootDocumentId"].as_str().unwrap()).unwrap();
    let requester =
        create_user_session(&run.harness, owner.workspace_id, WorkspaceRole::Member).await;
    add_project_member(
        &run.harness,
        owner.workspace_id,
        target,
        requester.user_id,
        "member",
    )
    .await;
    (addr, owner, requester, target, source, document)
}

#[tokio::test]
async fn personal_input_origin_revoke_first_after_source_read_denies_fresh_and_replay() {
    run_test("personal_input_origin_revoke_first_after_source_read_denies_fresh_and_replay",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);
    let(addr,owner,requester,target,source,document)=source_fixture(&mut run).await;let ws=owner.workspace_id;
    let path=document_api(ws,document,"/tasks");
    let replay_body=json!({"projectId":target,"requestId":Uuid::now_v7(),"task":{"title":"serialized replay"}});
    let(status,original)=session_call(addr,Method::POST,&path,&requester.session_token,Some(replay_body.clone())).await;assert_eq!(status,StatusCode::CREATED,"{original}");
    for body in [json!({"projectId":target,"requestId":Uuid::now_v7(),"task":{"title":"serialized fresh"}}),replay_body.clone()] {
      // TWO independent restricted control connections PLUS product request.
      let mut hold=observer(&run,ws).await;let hold_pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut hold).await.unwrap();
      sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR NO KEY UPDATE").bind(ws).bind(target).fetch_one(&mut hold).await.unwrap();
      let token=requester.session_token.clone();let request_path=path.clone();let request=tokio::spawn(async move{session_call(addr,Method::POST,&request_path,&token,Some(body)).await});
      let mut revoke=observer(&run,ws).await;
      let product_pid=blocked_by(&mut revoke,hold_pid).await;
      sqlx::query("SELECT pg_stat_clear_snapshot()").execute(&mut revoke).await.unwrap();
      let query:String=sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE pid=$1").bind(product_pid).fetch_one(&mut revoke).await.unwrap();
      assert!(query.contains("FOR NO KEY UPDATE") && query.contains("FROM fvoci.projects"),"barrier is target-lock after successful initial source-read: {query}");
      sqlx::query("UPDATE fvoci.projects SET visibility='private' WHERE workspace_id=$1 AND id=$2").bind(ws).bind(source).execute(&mut revoke).await.unwrap();
      sqlx::query("COMMIT").execute(&mut revoke).await.unwrap();
      let committed:String=sqlx::query_scalar("SELECT visibility FROM fvoci.projects WHERE workspace_id=$1 AND id=$2").bind(ws).bind(source).fetch_one(&mut hold).await.unwrap();assert_eq!(committed,"private");
      // The revocation committed while target control still held its lock.
      assert!(!request.is_finished());sqlx::query("COMMIT").execute(&mut hold).await.unwrap();
      let(status,value)=request.await.unwrap();assert_eq!(status,StatusCode::NOT_FOUND,"{value}");
      assert_eq!(committed_counts(&run,ws,"serialized fresh").await,(0,0,0,0,0,0,0));
      assert_eq!(committed_counts(&run,ws,"serialized replay").await,(0,1,1,1,1,1,0));
      // Restoring fixture visibility must commit on controlB before the next read.
      let mut restore=observer(&run,ws).await;
      sqlx::query("UPDATE fvoci.projects SET visibility='workspace' WHERE workspace_id=$1 AND id=$2").bind(ws).bind(source).execute(&mut restore).await.unwrap();
      sqlx::query("COMMIT").execute(&mut restore).await.unwrap();
      drop(hold);drop(revoke);
    }
    // Visible target + hidden source yields empty, with no source existence hint.
    let mut revoke=observer(&run,ws).await;sqlx::query("UPDATE fvoci.projects SET visibility='private' WHERE workspace_id=$1 AND id=$2").bind(ws).bind(source).execute(&mut revoke).await.unwrap();sqlx::query("COMMIT").execute(&mut revoke).await.unwrap();drop(revoke);
    let task=Uuid::parse_str(original["taskId"].as_str().unwrap()).unwrap();
    let(status,origin)=session_call(addr,Method::GET,&task_path(&requester,task,"/origin"),&requester.session_token,None).await;assert_eq!(status,StatusCode::OK,"{origin}");assert_eq!(origin["count"],0);assert_eq!(origin["items"],json!([]));assert!(!origin.to_string().contains(&document.to_string()));
    run.finish().await.unwrap();
 }).await;
}

#[tokio::test]
async fn personal_input_origin_operation_first_serializes_revocation_until_commit() {
    run_test("personal_input_origin_operation_first_serializes_revocation_until_commit",async {
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,owner,requester,target,source,document)=source_fixture(&mut run).await;let ws=owner.workspace_id;let command=Uuid::now_v7();let barrier=90010077i64;
    let admin=admin_pool(&run.harness).await;
    sqlx::raw_sql(&format!(r#"CREATE FUNCTION fvoci.fixture_origin_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.request_id='{command}'::uuid THEN PERFORM pg_advisory_xact_lock({barrier}); END IF; RETURN NEW; END $$;
    CREATE TRIGGER fixture_origin_barrier BEFORE INSERT ON fvoci.task_origins FOR EACH ROW EXECUTE FUNCTION fvoci.fixture_origin_barrier();"#)).execute(&admin).await.unwrap();
    let mut hold=observer(&run,ws).await;let hold_pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut hold).await.unwrap();sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(barrier).execute(&mut hold).await.unwrap();
    let token=requester.session_token.clone();let path=document_api(ws,document,"/tasks");let request=tokio::spawn(async move{session_call(addr,Method::POST,&path,&token,Some(json!({"projectId":target,"requestId":command,"task":{"title":"operation first"}}))).await});
    let mut revoke=observer(&run,ws).await;let product_pid=blocked_by(&mut revoke,hold_pid).await;let revoke_pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut revoke).await.unwrap();
    let revocation=tokio::spawn(async move {
        sqlx::query("UPDATE fvoci.projects SET visibility='private' WHERE workspace_id=$1 AND id=$2").bind(ws).bind(source).execute(&mut revoke).await.unwrap();
        sqlx::query("COMMIT").execute(&mut revoke).await.unwrap();
    });
    // Product is past final ACL and task/event/activity writes; independent
    // revocation must wait for its source project row lock, observed in PG.
    let waiting=blocked_by(&mut hold,product_pid).await;assert_eq!(waiting,revoke_pid);
    assert!(!revocation.is_finished());sqlx::query("COMMIT").execute(&mut hold).await.unwrap();
    let outcome=request.await.unwrap();revocation.await.unwrap();
    sqlx::raw_sql("DROP TRIGGER fixture_origin_barrier ON fvoci.task_origins; DROP FUNCTION fvoci.fixture_origin_barrier();").execute(&admin).await.unwrap();admin.close().await;
    assert_eq!(outcome.0,StatusCode::CREATED,"{}",outcome.1);assert_eq!(committed_counts(&run,ws,"operation first").await,(0,1,1,1,1,1,0));
    let(status,_)=session_call(addr,Method::POST,&document_api(ws,document,"/tasks"),&requester.session_token,Some(json!({"projectId":target,"requestId":command,"task":{"title":"operation first"}}))).await;assert_eq!(status,StatusCode::NOT_FOUND);
    run.finish().await.unwrap();
 }).await;
}

#[tokio::test]
async fn personal_input_start_survives_supported_team_slug_collision() {
    run_test("personal_input_start_survives_supported_team_slug_collision", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, fixture) = setup_task(&mut run, 5000).await;
        let actor = create_user_session(&run.harness, fixture.owner.workspace_id, WorkspaceRole::Member).await;
        let admin = admin_pool(&run.harness).await;
        // Fixture gives the caller the existing supported instance-admin role.
        sqlx::query("UPDATE fvoci.users SET is_instance_admin=true WHERE id=$1").bind(actor.user_id).execute(&admin).await.unwrap();
        admin.close().await;
        let slug = fvoci_server::db::workspace::personal_workspace_slug(actor.user_id);
        let (status, team) = session_call(addr, Method::POST, "/api/v1/workspaces", &actor.session_token, Some(json!({"slug":slug,"name":"Supported ordinary team"}))).await;
        assert_eq!(status, StatusCode::CREATED, "team API: {team}");
        let team_id = Uuid::parse_str(team["id"].as_str().unwrap()).unwrap();
        let mut conn = observer(&run,team_id).await;
        let kind:String=sqlx::query_scalar("SELECT kind FROM fvoci.workspaces WHERE id=$1").bind(team_id).fetch_one(&mut conn).await.unwrap();
        assert_eq!(kind,"team"); drop(conn);
        let (status, value) = session_call(addr, Method::POST, "/api/v1/me/personal-workspace", &actor.session_token,None).await;
        // Availability regression: a real supported team name must not block
        // private capture; the original failing response is retained externally.
        assert_eq!(status, StatusCode::OK, "personal start after supported team API: {value}");
        assert_ne!(value["id"],team_id.to_string());
        let personal_id = Uuid::parse_str(value["id"].as_str().unwrap()).unwrap();
        let address = value["slug"].as_str().unwrap();
        assert_ne!(address,slug); assert!(address.len()<=32);
        assert_eq!(fvoci_server::validate::normalize_slug(address).unwrap(),address);
        let (a,b)=tokio::join!(session_call(addr,Method::POST,"/api/v1/me/personal-workspace",&actor.session_token,None),session_call(addr,Method::POST,"/api/v1/me/personal-workspace",&actor.session_token,None));
        assert_eq!(a.0,StatusCode::OK); assert_eq!(b.0,StatusCode::OK);
        assert_eq!(a.1,value); assert_eq!(b.1,value);
        let mut conn=observer(&run,personal_id).await;
        let actual:(String,Uuid,i64)=sqlx::query_as("SELECT w.kind,u.personal_workspace_id,(SELECT count(*) FROM fvoci.memberships m WHERE m.workspace_id=w.id AND m.user_id=u.id AND m.role='owner') FROM fvoci.workspaces w JOIN fvoci.users u ON u.personal_workspace_id=w.id WHERE w.id=$1 AND u.id=$2").bind(personal_id).bind(actor.user_id).fetch_one(&mut conn).await.unwrap();
        assert_eq!(actual,("personal".into(),personal_id,1));drop(conn);
        let mut conn=observer(&run,team_id).await;
        let unchanged:(String,String,String)=sqlx::query_as("SELECT kind,slug,name FROM fvoci.workspaces WHERE id=$1").bind(team_id).fetch_one(&mut conn).await.unwrap();
        assert_eq!(unchanged,("team".into(),slug,"Supported ordinary team".into()));drop(conn);
        run.finish().await.unwrap();
    }).await;
}

#[tokio::test]
async fn personal_input_default_project_survives_archive_trash_and_key_collisions() {
    run_test("personal_input_default_project_survives_archive_trash_and_key_collisions", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5000).await;
        let owner = personal(addr, &f.owner).await;
        let ws = owner.workspace_id;
        let path = input_path(&owner);
        let project_path = |id: Uuid, suffix: &str| format!("/api/v1/workspaces/{ws}/projects/{id}{suffix}");
        let capture = |request: Uuid, title: &str| json!({"requestId":request,"intent":"task","title":title});
        let initial = capture(Uuid::now_v7(), "initial inbox");
        let (status, first) = session_call(addr, Method::POST, &path, &owner.session_token, Some(initial)).await;
        assert_eq!(status, StatusCode::CREATED, "{first}");
        let inbox = Uuid::parse_str(first["projectId"].as_str().unwrap()).unwrap();
        let (status, archived) = session_call(addr, Method::POST, &project_path(inbox, "/archive"), &owner.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{archived}");
        let after_archive = capture(Uuid::now_v7(), "after archived inbox");
        let (status, alternative) = session_call(addr, Method::POST, &path, &owner.session_token, Some(after_archive.clone())).await;
        // This genuine request fails at the reviewed pre-fix product SHA:
        // migration008 retains the archived INBOX unique key.
        assert_eq!(status, StatusCode::CREATED, "fresh capture after user archive must succeed: {alternative}");
        let alternate = Uuid::parse_str(alternative["projectId"].as_str().unwrap()).unwrap();
        assert_ne!(alternate, inbox);
        let (a, b) = tokio::join!(
            session_call(addr, Method::POST, &path, &owner.session_token, Some(after_archive.clone())),
            session_call(addr, Method::POST, &path, &owner.session_token, Some(after_archive))
        );
        assert_eq!(a.0, StatusCode::CREATED, "{}", a.1);
        assert_eq!(b.0, StatusCode::CREATED, "{}", b.1);
        assert_eq!(a.1["taskId"], alternative["taskId"]);
        assert_eq!(b.1["documentId"], alternative["documentId"]);
        assert_eq!(committed_counts(&run, ws, "after archived inbox").await, (1,1,1,1,1,1,1));
        let (status, trashed) = session_call(addr, Method::DELETE, &project_path(alternate, ""), &owner.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{trashed}");
        let (status, third) = session_call(addr, Method::POST, &path, &owner.session_token, Some(capture(Uuid::now_v7(), "after trashed alternative"))).await;
        assert_eq!(status, StatusCode::CREATED, "{third}");
        let active = Uuid::parse_str(third["projectId"].as_str().unwrap()).unwrap();
        assert_ne!(active, alternate);
        let (status, reuse) = session_call(addr, Method::POST, &path, &owner.session_token, Some(capture(Uuid::now_v7(), "reuse active private alternative"))).await;
        assert_eq!(status, StatusCode::CREATED, "{reuse}");
        assert_eq!(reuse["projectId"], third["projectId"]);
        let mut conn = observer(&run, ws).await;
        let rows: Vec<(Uuid, String, String, String, bool)> = sqlx::query_as("SELECT id,key,status,visibility,deleted_at IS NOT NULL FROM fvoci.projects WHERE workspace_id=$1 ORDER BY key")
            .bind(ws).fetch_all(&mut conn).await.unwrap();
        assert!(rows.iter().any(|row| row == &(inbox, "INBOX".into(), "archived".into(), "private".into(), false)));
        assert!(rows.iter().any(|row| row.0 == alternate && row.1 == "INBOX-A" && row.4));
        assert!(rows.iter().any(|row| row == &(active, "INBOX-B".into(), "active".into(), "private".into(), false)));
        drop(conn);
        let (status, value) = session_call(addr, Method::POST, &project_path(active, "/archive"), &owner.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let collision_request = Uuid::now_v7();
        // Ordinary project commands can occupy every human key and a predicted
        // request-derived key; the allocator must inspect, not restore/purge.
        let suffix = collision_request.simple().to_string()[8..].to_uppercase();
        for key in ["INBOX-C".to_string(), format!("INBOX-D{suffix}")] {
            let occupied = create_project(addr, &owner, &key).await;
            let (status, value) = session_call(addr, Method::POST, &project_path(occupied, "/archive"), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        let collision = capture(collision_request, "skip occupied fallback key");
        let (status, skipped) = session_call(addr, Method::POST, &path, &owner.session_token, Some(collision.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{skipped}");
        let selected = Uuid::parse_str(skipped["projectId"].as_str().unwrap()).unwrap();
        let mut conn = observer(&run, ws).await;
        let key: String = sqlx::query_scalar("SELECT key FROM fvoci.projects WHERE workspace_id=$1 AND id=$2").bind(ws).bind(selected).fetch_one(&mut conn).await.unwrap();
        assert_eq!(key, format!("INBOX-E{suffix}"));
        drop(conn);
        let (status, replay) = session_call(addr, Method::POST, &path, &owner.session_token, Some(collision)).await;
        assert_eq!(status, StatusCode::CREATED, "{replay}");
        assert_eq!(replay["taskId"], skipped["taskId"]);
        assert_eq!(committed_counts(&run, ws, "skip occupied fallback key").await, (1,1,1,1,1,1,1));
        let (status, value) = session_call(addr, Method::POST, &project_path(selected, "/archive"), &owner.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let exhausted_request = Uuid::now_v7();
        let suffix = exhausted_request.simple().to_string()[8..].to_uppercase();
        for prefix in ['D', 'E', 'F', 'G'] {
            let occupied = create_project(addr, &owner, &format!("INBOX-{prefix}{suffix}")).await;
            let (status, value) = session_call(addr, Method::POST, &project_path(occupied, "/archive"), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{value}");
        }
        let before = default_project_counts(&run, ws).await;
        let command = capture(exhausted_request, "bounded allocation failure");
        let (status, value) = session_call(addr, Method::POST, &path, &owner.session_token, Some(command.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "all occupied keys must fail without any partial write: {value}");
        assert_eq!(default_project_counts(&run, ws).await, before);
        assert_eq!(committed_counts(&run, ws, "bounded allocation failure").await, (0,0,0,0,0,0,0));
        // A later explicit user unarchive supplies an eligible ordinary target;
        // retrying the failed command is permitted because no receipt committed.
        let (status, value) = session_call(addr, Method::POST, &project_path(active, "/unarchive"), &owner.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let (status, recovered) = session_call(addr, Method::POST, &path, &owner.session_token, Some(command)).await;
        assert_eq!(status, StatusCode::CREATED, "{recovered}");
        assert_eq!(recovered["projectId"], active.to_string());
        assert_eq!(committed_counts(&run, ws, "bounded allocation failure").await, (1,1,1,1,1,1,1));
        run.finish().await.unwrap();
    }).await;
}

async fn default_project_counts(run: &TestRun, ws: Uuid) -> (i64, i64, i64, i64, i64) {
    let mut conn = observer(run, ws).await;
    sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1), (SELECT count(*) FROM fvoci.workflows WHERE workspace_id=$1), (SELECT count(*) FROM fvoci.statuses WHERE workspace_id=$1), (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1), (SELECT count(*) FROM fvoci.personal_input_commands WHERE workspace_id=$1)")
        .bind(ws).fetch_one(&mut conn).await.unwrap()
}
