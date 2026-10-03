//! SQL/RLS/trigger witness for the first ordinary graph. Native/history/file
//! completion remains a separate required witness, never inferred from this.
use super::*;
use sqlx::{Connection, PgConnection};

#[derive(Debug)]
struct ObserverRelation {
    relname: String,
    owner: String,
    owner_member: bool,
    enabled: bool,
    forced: bool,
    active: bool,
}
#[derive(Debug)]
struct ObserverMetadata {
    database: String,
    role: String,
    superuser: bool,
    bypass: bool,
    relations: Vec<ObserverRelation>,
}
async fn observer_metadata(conn: &mut PgConnection) -> ObserverMetadata {
    let (database, role, superuser, bypass): (String, String, bool, bool) = sqlx::query_as(
        "SELECT current_database(),current_user,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
    ).fetch_one(&mut *conn).await.unwrap();
    let rows: Vec<(String, String, bool, bool, bool, bool)> = sqlx::query_as("SELECT relname,pg_get_userbyid(relowner) AS owner,pg_has_role(current_user,relowner,'MEMBER') AS owner_member,relrowsecurity AS enabled,relforcerowsecurity AS forced,row_security_active(oid) AS active FROM pg_class WHERE oid=ANY(ARRAY['fvoci.documents'::regclass,'fvoci.tasks'::regclass,'fvoci.personal_transfer_commands'::regclass]) ORDER BY relname")
        .fetch_all(&mut *conn).await.unwrap();
    let relations = rows
        .into_iter()
        .map(
            |(relname, owner, owner_member, enabled, forced, active)| ObserverRelation {
                relname,
                owner,
                owner_member,
                enabled,
                forced,
                active,
            },
        )
        .collect();
    ObserverMetadata {
        database,
        role,
        superuser,
        bypass,
        relations,
    }
}
fn restricted_observer(metadata: &ObserverMetadata) -> bool {
    !metadata.superuser
        && !metadata.bypass
        && metadata.relations.len() == 3
        && metadata
            .relations
            .iter()
            .zip([
                ("documents", false),
                ("personal_transfer_commands", true),
                ("tasks", true),
            ])
            .all(|(relation, (name, forced))| {
                relation.relname == name
                    && relation.owner != metadata.role
                    && !relation.owner_member
                    && relation.enabled
                    && relation.active
                    && relation.forced == forced
            })
}
async fn scoped_observer(run: &TestRun, ws: Uuid) -> PgConnection {
    let mut conn = PgConnection::connect(&run.harness.app_url).await.unwrap();
    let metadata = observer_metadata(&mut conn).await;
    assert_eq!(
        metadata.database,
        run.harness.db_name(),
        "observer must use this isolated fixture database"
    );
    assert!(
        restricted_observer(&metadata),
        "observer isolation contract: {metadata:?}"
    );
    eprintln!("transfer role witness: {metadata:?}");
    sqlx::query("BEGIN").execute(&mut conn).await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id',$1,true)")
        .bind(ws.to_string())
        .execute(&mut conn)
        .await
        .unwrap();
    conn
}
async fn personal_source(addr: SocketAddr, team: &SessionFixture) -> Uuid {
    // The existing fixture's team uses a personal-shaped slug. Rename only
    // that fixture; then exercise the real owner-only personal endpoint.
    let mut tx = team.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, team.workspace_id)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.workspaces SET slug=$2 WHERE id=$1 AND kind='team'")
        .bind(team.workspace_id)
        .bind(format!(
            "team-{}",
            &team.workspace_id.simple().to_string()[20..]
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, value) = session_call(
        addr,
        Method::POST,
        "/api/v1/me/personal-workspace",
        &team.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        value["slug"],
        fvoci_server::db::workspace::personal_workspace_slug(team.user_id)
    );
    let id = Uuid::parse_str(value["id"].as_str().unwrap()).unwrap();
    assert_ne!(id, team.workspace_id);
    id
}
async fn setup_transfer(
    run: &mut TestRun,
) -> (SocketAddr, SessionFixture, Uuid, Uuid, Value, Value) {
    let (addr, fixture) = setup_task(run, 5000).await;
    let actor = fixture.owner;
    let (source, project, create, selection) = transfer_pair(addr, &actor).await;
    (addr, actor, source, project, create, selection)
}

/// The captured personal pair and a MOVE selection into a new team project.
async fn transfer_pair(addr: SocketAddr, actor: &SessionFixture) -> (Uuid, Uuid, Value, Value) {
    let source = personal_source(addr, actor).await;
    let project = create_named_project(addr, actor, "PUBLISH", "workspace").await;
    let create = json!({"requestId":Uuid::now_v7(),"intent":"task","title":"원본 한글 🧑‍💻"});
    let (status, pair) = session_call(
        addr,
        Method::POST,
        &format!("/api/v1/workspaces/{source}/personal-input"),
        &actor.session_token,
        Some(create.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pair}");
    let (status, workflow) = session_call(
        addr,
        Method::GET,
        &format!(
            "/api/v1/workspaces/{}/projects/{project}/workflow",
            actor.workspace_id
        ),
        &actor.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{workflow}");
    let status_id = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|status| status["category"] == "backlog")
        .unwrap()["id"]
        .clone();
    let selection = json!({"action":"move","documentId":pair["documentId"],"taskId":pair["taskId"],"expectedDocumentVersion":1,"expectedTaskVersion":1,"destinationWorkspaceId":actor.workspace_id,"destinationProjectId":project,"destinationStatusId":status_id});
    (source, project, create, selection)
}
fn path(source: Uuid) -> String {
    format!("/api/v1/workspaces/{source}/personal-transfers")
}
async fn command(
    addr: SocketAddr,
    actor: &SessionFixture,
    source: Uuid,
    selection: &Value,
) -> Value {
    let (status, preview) = session_call(
        addr,
        Method::POST,
        &format!("{}/preview", path(source)),
        &actor.session_token,
        Some(selection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["projectVisibility"], "workspace");
    assert_eq!(preview["documentTitle"], "원본 한글 🧑‍💻");
    assert_eq!(preview["sourceRetained"], selection["action"] == "copy");
    // Typed disclosure graph: the pair's outcome follows the action, and
    // nothing refused today (history/files on MOVE) is ever claimed as moved.
    let carried = if selection["action"] == "copy" {
        "copied_new_id"
    } else {
        "moved"
    };
    let dispositions = preview["dispositions"].as_array().unwrap();
    assert_eq!(
        dispositions[0],
        json!({"item":"document","outcome":carried,"count":1})
    );
    assert_eq!(
        dispositions[1],
        json!({"item":"task","outcome":carried,"count":1})
    );
    assert!(dispositions.iter().all(|d| d["item"] != "attachment"
        && !(d["item"] == "history" && d["outcome"] != "retained_private")));
    json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true})
}
async fn counts(
    run: &TestRun,
    ws: Uuid,
    document: Uuid,
    task: Uuid,
) -> (i64, i64, i64, i64, i64, i64, i64) {
    let mut conn = scoped_observer(run, ws).await;
    let counts=sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1 AND id=$2),(SELECT count(*) FROM fvoci.tasks WHERE workspace_id=$1 AND id=$3),(SELECT count(*) FROM fvoci.task_origins WHERE workspace_id=$1 AND task_id=$3 AND document_id=$2),(SELECT count(*) FROM fvoci.task_assignees WHERE workspace_id=$1 AND task_id=$3),(SELECT count(*) FROM fvoci.task_activity WHERE workspace_id=$1 AND task_id=$3),(SELECT count(*) FROM fvoci.collection_items WHERE workspace_id=$1 AND task_id=$3),(SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND target_id=ANY(ARRAY[$2,$3]))").bind(ws).bind(document).bind(task).fetch_one(&mut conn).await.unwrap();
    sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    counts
}

/// Fresh committed app-role graph, including native bytes/history, ACL,
/// counters, capture receipts, audit and outbox; transfer receipts separately.
async fn transfer_graph(run: &TestRun, workspace: Uuid) -> Value {
    let mut conn = scoped_observer(run, workspace).await;
    let mut graph = serde_json::Map::new();
    for table in [
        "documents",
        "tasks",
        "document_states",
        "task_states",
        "document_collab_updates",
        "task_collab_updates",
        "document_collab_op_receipts",
        "task_collab_op_receipts",
        "revisions",
        "task_origins",
        "task_assignees",
        "task_activity",
        "collection_items",
        "projects",
        "memberships",
        "document_members",
        "attachments",
        "events",
        "audit_log",
        "personal_input_commands",
    ] {
        let sql=format!("SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM fvoci.{table} r WHERE workspace_id=$1");
        let rows: Value = sqlx::query_scalar(&sql)
            .bind(workspace)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        graph.insert(table.into(), rows);
    }
    sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    Value::Object(graph)
}

fn in_personal(actor: &SessionFixture, source: Uuid) -> SessionFixture {
    SessionFixture {
        workspace_id: source,
        pool: actor.pool.clone(),
        user_id: actor.user_id,
        session_id: actor.session_id,
        session_token: actor.session_token.clone(),
    }
}

async fn selection_versions(run: &TestRun, source: Uuid, selection: &mut Value) {
    let mut conn = scoped_observer(run, source).await;
    let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
    let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
    selection["expectedDocumentVersion"] = json!(sqlx::query_scalar::<_, i32>(
        "SELECT version FROM fvoci.documents WHERE workspace_id=$1 AND id=$2"
    )
    .bind(source)
    .bind(document)
    .fetch_one(&mut conn)
    .await
    .unwrap());
    selection["expectedTaskVersion"] = json!(sqlx::query_scalar::<_, i32>(
        "SELECT version FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2"
    )
    .bind(source)
    .bind(task)
    .fetch_one(&mut conn)
    .await
    .unwrap());
    sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
}

async fn committed_transfer_hints(
    run: &TestRun,
    ws: Uuid,
    project: Uuid,
    task: Uuid,
    verb: &str,
) -> Vec<Option<(String, String)>> {
    let mut conn = scoped_observer(run, ws).await;
    let rows: Vec<(String, i64, String, Value)> = sqlx::query_as("SELECT xact::text,seq,verb,payload FROM fvoci.events WHERE workspace_id=$1 AND target_id=$2 AND verb=$3 ORDER BY xact,seq")
        .bind(ws).bind(task).bind(verb).fetch_all(&mut conn).await.unwrap();
    sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    rows.into_iter()
        .map(|(xact, seq, verb, payload)| {
            assert_eq!(payload["projectId"], project.to_string());
            fvoci_server::streams::task_stream_wire_hint(&fvoci_server::streams::StreamEventRow {
                xact,
                seq,
                verb,
                payload,
            })
        })
        .collect()
}

#[tokio::test]
async fn personal_transfer_same_ids_real_rls_triggers_and_repeat() {
    run_test("personal_transfer_same_ids_real_rls_triggers_and_repeat",async{
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,actor,source,project,capture,selection)=setup_transfer(&mut run).await;
    // Independent preparation connection is a negative control ONLY. It reads
    // catalogs, never transfer content or product requests; superuser/RLS-
    // inactive metadata must not be accepted as a committed app-role witness.
    let mut preparation = PgConnection::connect(&run.harness.admin_url).await.unwrap();
    let invalid_observer = observer_metadata(&mut preparation).await;
    assert!(invalid_observer.superuser || invalid_observer.bypass);
    assert!(invalid_observer.relations.iter().all(|relation| !relation.active));
    assert!(!restricted_observer(&invalid_observer));
    eprintln!("rejected preparation observer negative control: {invalid_observer:?}");
    preparation.close().await.unwrap();
    let doc=Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();let task=Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
    let before=counts(&run,source,doc,task).await;assert_eq!((before.0,before.1,before.2,before.3,before.4,before.5),(1,1,1,1,1,1));
    let mut conn=scoped_observer(&run,source).await;
    // Direct tenant UPDATE is not the implementation: FORCE RLS rejects it.
    let error=sqlx::query("UPDATE fvoci.tasks SET workspace_id=$3 WHERE workspace_id=$1 AND id=$2").bind(source).bind(task).bind(actor.workspace_id).execute(&mut conn).await.unwrap_err();assert_eq!(error.as_database_error().unwrap().code().as_deref(),Some("23514"));assert!(error.to_string().contains("detach task before changing scope"));sqlx::query("ROLLBACK").execute(&mut conn).await.unwrap();drop(conn);
    let mut conn=scoped_observer(&run,source).await;
    // Isolate WITH CHECK from the separate populated-collection trigger.
    sqlx::query("DELETE FROM fvoci.collection_items WHERE workspace_id=$1 AND task_id=$2").bind(source).bind(task).execute(&mut conn).await.unwrap();
    let error=sqlx::query("UPDATE fvoci.tasks SET workspace_id=$3 WHERE workspace_id=$1 AND id=$2").bind(source).bind(task).bind(actor.workspace_id).execute(&mut conn).await.unwrap_err();assert_eq!(error.as_database_error().unwrap().code().as_deref(),Some("42501"));sqlx::query("ROLLBACK").execute(&mut conn).await.unwrap();drop(conn);
    let body=command(addr,&actor,source,&selection).await;
    assert_eq!(counts(&run,source,doc,task).await,before,"preview/Cancel means no command and no effects");
    let mut conn=scoped_observer(&run,source).await;
    let source_project: Uuid = sqlx::query_scalar("SELECT project_id FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2").bind(source).bind(task).fetch_one(&mut conn).await.unwrap();
    let baseline:(Uuid,Uuid,Value)=sqlx::query_as("SELECT i.id,a.id,a.changes FROM fvoci.collection_items i JOIN fvoci.task_activity a ON a.workspace_id=i.workspace_id AND a.task_id=i.task_id WHERE i.workspace_id=$1 AND i.task_id=$2").bind(source).bind(task).fetch_one(&mut conn).await.unwrap();sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    // Receive actual status then discard successful body. A fresh connection
    // establishes commit before exact repeated/concurrent product commands.
    let response=reqwest::Client::new().post(format!("http://{addr}{}",path(source))).header("origin",PUBLIC_ORIGIN).header("cookie",format!("fvoci_session={}",actor.session_token)).json(&body).send().await.unwrap();assert_eq!(response.status(),StatusCode::OK);drop(response);
    let committed=counts(&run,actor.workspace_id,doc,task).await;assert_eq!((committed.0,committed.1,committed.2,committed.3,committed.4,committed.5),(1,1,1,1,1,1));
    let replay_path=path(source);
    let a=session_call(addr,Method::POST,&replay_path,&actor.session_token,Some(body.clone()));let b=session_call(addr,Method::POST,&replay_path,&actor.session_token,Some(body.clone()));let(a,b)=tokio::join!(a,b);
    for(status,value)in[a,b]{assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["documentId"],doc.to_string());assert_eq!(value["taskId"],task.to_string());assert_eq!(value["replayed"],true);}
    assert_eq!(counts(&run,actor.workspace_id,doc,task).await,committed);
    // These are real committed event rows converted by the production wire
    // function, after both repeated requests. Exact one hint per scope.
    let source_hints = committed_transfer_hints(&run, source, source_project, task, "task.deleted").await;
    let destination_hints = committed_transfer_hints(&run, actor.workspace_id, project, task, "task.created").await;
    assert_eq!([source_hints, destination_hints], [vec![Some(("task.deleted".into(), task.to_string()))], vec![Some(("task.created".into(), task.to_string()))]]);

    let mut conn=scoped_observer(&run,actor.workspace_id).await;
    let moved:(Uuid,Uuid,Value)=sqlx::query_as("SELECT i.id,a.id,a.changes FROM fvoci.collection_items i JOIN fvoci.task_activity a ON a.workspace_id=i.workspace_id AND a.task_id=i.task_id WHERE i.workspace_id=$1 AND i.task_id=$2").bind(actor.workspace_id).bind(task).fetch_one(&mut conn).await.unwrap();assert_eq!(moved,baseline,"same stable collection item and inert activity, no replay");
    let mapped:(Uuid,Uuid)=sqlx::query_as("SELECT t.project_id,d.project_id FROM fvoci.tasks t JOIN fvoci.documents d ON d.workspace_id=t.workspace_id WHERE t.workspace_id=$1 AND t.id=$2 AND d.id=$3").bind(actor.workspace_id).bind(task).bind(doc).fetch_one(&mut conn).await.unwrap();assert_eq!(mapped,(project,project));sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    let mut conn=scoped_observer(&run,source).await;let retired:(i64,i64,i64)=sqlx::query_as("SELECT count(*),count(document_id),count(task_id) FROM fvoci.personal_input_commands WHERE workspace_id=$1 AND request_id=$2").bind(source).bind(Uuid::parse_str(capture["requestId"].as_str().unwrap()).unwrap()).fetch_one(&mut conn).await.unwrap();assert_eq!(retired,(1,0,0));let receipts:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.personal_transfer_commands WHERE workspace_id=$1 AND request_id=$2").bind(source).bind(Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap()).fetch_one(&mut conn).await.unwrap();assert_eq!(receipts,1);sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    let(status,_)=session_call(addr,Method::POST,&format!("/api/v1/workspaces/{source}/personal-input"),&actor.session_token,Some(capture)).await;assert_eq!(status,StatusCode::NOT_FOUND);
    let mut changed=body.clone();changed["selection"]["expectedDocumentVersion"]=json!(2);let(status,error)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(changed)).await;assert_eq!(status,StatusCode::CONFLICT,"{error}");assert_eq!(error["code"],"personal_transfer_conflict");assert_eq!(error["params"]["code"],"command_changed");
    let outsider=create_user_session(&run.harness,actor.workspace_id,WorkspaceRole::Member).await;
    let(status,_)=session_call(addr,Method::GET,&document_api(source,doc,""),&outsider.session_token,None).await;assert_eq!(status,StatusCode::NOT_FOUND);
    let(status,visible)=session_call(addr,Method::GET,&task_path(&actor,task,""),&outsider.session_token,None).await;assert_eq!(status,StatusCode::OK,"{visible}");assert_eq!(visible["id"],task.to_string());
    use fvoci_server::streams::{project_stream_access, StreamAccess};
    assert_eq!(project_stream_access(&actor.pool, source, source_project, outsider.user_id, outsider.session_id).await.unwrap(), StreamAccess::Denied);
    assert_eq!(project_stream_access(&actor.pool, actor.workspace_id, project, outsider.user_id, outsider.session_id).await.unwrap(), StreamAccess::Allowed);
    let (status, changed) = session_call(addr, Method::PATCH, &format!("/api/v1/workspaces/{}/projects/{project}", actor.workspace_id), &actor.session_token, Some(json!({"visibility":"private"}))).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(project_stream_access(&actor.pool, actor.workspace_id, project, outsider.user_id, outsider.session_id).await.unwrap(), StreamAccess::Denied);

    run.finish().await.unwrap();}).await;
}

#[tokio::test]
async fn personal_transfer_copy_is_new_identity_and_original_stays_private() {
    run_test(
        "personal_transfer_copy_is_new_identity_and_original_stays_private",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
            selection["action"] = json!("copy");
            let doc = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
            let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
            let before = counts(&run, source, doc, task).await;
            let body = command(addr, &actor, source, &selection).await;
            let (status, copy) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{copy}");
            assert_ne!(copy["documentId"], doc.to_string());
            assert_ne!(copy["taskId"], task.to_string());
            assert_eq!(counts(&run, source, doc, task).await, before);
            let (status, replay) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{replay}");
            assert_eq!(replay["documentId"], copy["documentId"]);
            assert_eq!(replay["taskId"], copy["taskId"]);
            assert_eq!(replay["replayed"], true);
            let outsider =
                create_user_session(&run.harness, actor.workspace_id, WorkspaceRole::Member).await;
            let (status, _) = session_call(
                addr,
                Method::GET,
                &document_api(source, doc, ""),
                &outsider.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_receipt_failure_rolls_back_both_graphs_and_triggers() {
    run_test("personal_transfer_receipt_failure_rolls_back_both_graphs_and_triggers",async{
    let mut run=TestRun::new(TestDb::bootstrap().await);let(addr,actor,source,_,capture,selection)=setup_transfer(&mut run).await;let doc=Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();let task=Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();let source_before=counts(&run,source,doc,task).await;let target_before=counts(&run,actor.workspace_id,doc,task).await;let body=command(addr,&actor,source,&selection).await;
    let request=Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap();let admin=admin_pool(&run.harness).await;
    sqlx::raw_sql(&format!(r#"CREATE FUNCTION fvoci.fixture_transfer_receipt_fail() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,fvoci AS $$ BEGIN
      IF NEW.request_id='{request}'::uuid AND EXISTS(SELECT 1 FROM fvoci.task_activity WHERE workspace_id=NEW.destination_workspace_id AND task_id=NEW.task_id) AND EXISTS(SELECT 1 FROM fvoci.events WHERE workspace_id=NEW.destination_workspace_id AND target_id=NEW.task_id AND verb='task.created') THEN RAISE EXCEPTION 'fixture_transfer_after_activity_event_before_receipt'; END IF;RETURN NEW;END $$;
      CREATE TRIGGER fixture_transfer_receipt_fail BEFORE INSERT ON fvoci.personal_transfer_commands FOR EACH ROW EXECUTE FUNCTION fvoci.fixture_transfer_receipt_fail();"#)).execute(&admin).await.unwrap();
    let(status,_)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::INTERNAL_SERVER_ERROR);
    sqlx::raw_sql("DROP TRIGGER fixture_transfer_receipt_fail ON fvoci.personal_transfer_commands;DROP FUNCTION fvoci.fixture_transfer_receipt_fail();").execute(&admin).await.unwrap();admin.close().await;
    assert_eq!(counts(&run,source,doc,task).await,source_before);assert_eq!(counts(&run,actor.workspace_id,doc,task).await,target_before);
    let mut conn=scoped_observer(&run,source).await;let untouched:(Uuid,Uuid)=sqlx::query_as("SELECT document_id,task_id FROM fvoci.personal_input_commands WHERE workspace_id=$1 AND request_id=$2").bind(source).bind(Uuid::parse_str(capture["requestId"].as_str().unwrap()).unwrap()).fetch_one(&mut conn).await.unwrap();assert_eq!(untouched,(doc,task));let receipts:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.personal_transfer_commands WHERE workspace_id=$1 AND request_id=$2").bind(source).bind(request).fetch_one(&mut conn).await.unwrap();assert_eq!(receipts,0);sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
    let(status,retry)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(body)).await;assert_eq!(status,StatusCode::OK,"{retry}");assert_eq!(retry["documentId"],doc.to_string());run.finish().await.unwrap();}).await;
}

#[tokio::test]
async fn personal_transfer_attachment_old_key_trap_and_fresh_key_survival() {
    run_test("personal_transfer_attachment_old_key_trap_and_fresh_key_survival", async {
        let mut run=TestRun::new(TestDb::bootstrap().await);
        let(addr,actor,source,project,_,selection)=setup_transfer(&mut run).await;
        let source_task=Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let destination_task=create_task(addr,&actor,project).await;
        let root=std::env::temp_dir().join(format!("fvoci-w2-transfer-key-proof-{}",Uuid::now_v7()));
        let storage=fvoci_server::attachments::ObjectStorage::local(root.clone());
        let payload=b"independent immutable original bytes".to_vec();
        for fresh in [false,true] {
            let attachment=Uuid::now_v7();let old_key=Uuid::now_v7().to_string();
            let destination_key=if fresh{Uuid::now_v7().to_string()}else{old_key.clone()};
            storage.put_bytes(&old_key,payload.clone()).await.unwrap();
            if fresh {storage.put_bytes(&destination_key,payload.clone()).await.unwrap();}
            let mut conn=scoped_observer(&run,source).await;
            sqlx::query("INSERT INTO fvoci.attachments(id,workspace_id,task_id,uploader_id,status,name,mime,reserved_size_bytes,size_bytes,storage_key,scan_status,completed_at) VALUES($1,$2,$3,$4,'stored','proof.bin','application/octet-stream',$5,$5,$6,'clean',now())").bind(attachment).bind(source).bind(source_task).bind(actor.user_id).bind(payload.len() as i64).bind(&old_key).execute(&mut conn).await.unwrap();
            sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2").bind(source).bind(attachment).execute(&mut conn).await.unwrap();
            sqlx::query("SELECT set_config('app.tenant_id',$1,true)").bind(actor.workspace_id.to_string()).execute(&mut conn).await.unwrap();
            sqlx::query("INSERT INTO fvoci.attachments(id,workspace_id,task_id,uploader_id,status,name,mime,reserved_size_bytes,size_bytes,storage_key,scan_status,completed_at) VALUES($1,$2,$3,$4,'stored','proof.bin','application/octet-stream',$5,$5,$6,'clean',now())").bind(attachment).bind(actor.workspace_id).bind(destination_task).bind(actor.user_id).bind(payload.len() as i64).bind(&destination_key).execute(&mut conn).await.unwrap();
            sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
            // Existing maintenance entry point's system context is for its
            // cleanup ledger only; no transfer content RLS bypass is added.
            let stats=fvoci_server::db::attachments::reclaim_attachment_objects(&actor.pool,&storage,Some((source,attachment)),10).await.unwrap();
            assert_eq!((stats.claimed,stats.reclaimed,stats.busy,stats.failed),(1,1,0,0));
            assert_eq!(storage.head(&old_key).await.unwrap(),None);
            let mut conn=scoped_observer(&run,actor.workspace_id).await;
            let key:String=sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE workspace_id=$1 AND id=$2").bind(actor.workspace_id).bind(attachment).fetch_one(&mut conn).await.unwrap();assert_eq!(key,destination_key);
            sqlx::query("COMMIT").execute(&mut conn).await.unwrap();drop(conn);
            if fresh {
                assert_eq!(storage.head(&destination_key).await.unwrap(),Some(payload.len() as u64));
                assert_eq!(storage.read_range(&destination_key,0,payload.len() as u64-1).await.unwrap(),payload);
            } else {
                assert_eq!(storage.head(&destination_key).await.unwrap(),None,"negative control: a committed destination attachment row is insufficient when the old source cleanup key is reused");
            }
        }
        run.finish().await.unwrap();std::fs::remove_dir_all(root).unwrap();
    }).await;
}

#[tokio::test]
async fn personal_transfer_stale_metadata_preview_and_declined_confirmation_have_no_effects() {
    run_test(
        "personal_transfer_stale_metadata_preview_and_declined_confirmation_have_no_effects",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, selection) = setup_transfer(&mut run).await;
            let doc = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
            let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
            let mut body = command(addr, &actor, source, &selection).await;
            let before = counts(&run, source, doc, task).await;
            body["confirmed"] = json!(false);
            let (status, _) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body.clone()),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(counts(&run, source, doc, task).await, before);
            body["confirmed"] = json!(true);
            let source_actor = SessionFixture {
                workspace_id: source,
                pool: actor.pool.clone(),
                user_id: actor.user_id,
                session_id: actor.session_id,
                session_token: actor.session_token.clone(),
            };
            let (status, patched) = session_call(
                addr,
                Method::PATCH,
                &task_path(&source_actor, task, ""),
                &actor.session_token,
                Some(json!({"title":"編集中の別のタイトル"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{patched}");
            assert_eq!(
                patched["version"], 1,
                "ordinary metadata PATCH does not advance native body version"
            );
            let after_patch = counts(&run, source, doc, task).await;
            let (status, error) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["code"], "personal_transfer_conflict");
            assert_eq!(error["params"]["code"], "preview_stale");
            assert_eq!(counts(&run, source, doc, task).await, after_patch);
            let target = counts(&run, actor.workspace_id, doc, task).await;
            assert_eq!(target, (0, 0, 0, 0, 0, 0, 0));
            run.finish().await.unwrap();
        },
    )
    .await;
}

async fn transfer_blocked_by(conn: &mut PgConnection, blocker: i32) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        sqlx::query("SELECT pg_stat_clear_snapshot()")
            .execute(&mut *conn)
            .await
            .unwrap();
        let waiting: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE pid<>pg_backend_pid() AND $1=ANY(pg_blocking_pids(pid)) AND usename=current_user ORDER BY pid LIMIT 1")
            .bind(blocker).fetch_optional(&mut *conn).await.unwrap();
        if let Some(pid) = waiting {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no observed product waiter for blocker {blocker}"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn personal_transfer_source_revoke_first_after_source_read_denies_fresh_and_replay() {
    run_test("personal_transfer_source_revoke_first_after_source_read_denies_fresh_and_replay", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, project, _, selection) = setup_transfer(&mut run).await;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let body = command(addr, &actor, source, &selection).await;
        for replay in [false, true] {
            if replay {
                let mut restore = scoped_observer(&run, source).await;
                let changed = sqlx::query("UPDATE fvoci.memberships SET role='owner' WHERE workspace_id=$1 AND user_id=$2")
                    .bind(source).bind(actor.user_id).execute(&mut restore).await.unwrap();
                assert_eq!(changed.rows_affected(), 1);
                sqlx::query("COMMIT").execute(&mut restore).await.unwrap();
                let (status, original) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body.clone())).await;
                assert_eq!(status, StatusCode::OK, "{original}");
            }
            let source_before = counts(&run, source, document, task).await;
            let destination_before = counts(&run, actor.workspace_id, document, task).await;
            // Control A holds only the destination project lock. Product has
            // passed the successful source graph/receipt-owner read first.
            let mut hold = scoped_observer(&run, actor.workspace_id).await;
            let hold_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut hold).await.unwrap();
            sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR NO KEY UPDATE")
                .bind(actor.workspace_id).bind(project).fetch_one(&mut hold).await.unwrap();
            let request_path = path(source);
            let token = actor.session_token.clone();
            let payload = body.clone();
            let request = tokio::spawn(async move {
                session_call(addr, Method::POST, &request_path, &token, Some(payload)).await
            });
            // Independent control B observes the real backend wait, then
            // commits the source permission revocation before A releases.
            let mut revoke = scoped_observer(&run, source).await;
            let revoke_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut revoke).await.unwrap();
            let product_pid = transfer_blocked_by(&mut revoke, hold_pid).await;
            assert_ne!(hold_pid, revoke_pid);
            assert_ne!(product_pid, hold_pid);
            assert_ne!(product_pid, revoke_pid);
            let query: String = sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE pid=$1")
                .bind(product_pid).fetch_one(&mut revoke).await.unwrap();
            assert!(query.contains("FROM fvoci.projects") && query.contains("FOR NO KEY UPDATE"), "source-read/target-lock barrier: {query}");
            let changed = sqlx::query("UPDATE fvoci.memberships SET role='guest' WHERE workspace_id=$1 AND user_id=$2")
                .bind(source).bind(actor.user_id).execute(&mut revoke).await.unwrap();
            assert_eq!(changed.rows_affected(), 1);
            sqlx::query("COMMIT").execute(&mut revoke).await.unwrap();
            sqlx::query("SELECT set_config('app.tenant_id',$1,true)")
                .bind(source.to_string()).execute(&mut hold).await.unwrap();
            let committed_role: String = sqlx::query_scalar("SELECT role FROM fvoci.memberships WHERE workspace_id=$1 AND user_id=$2")
                .bind(source).bind(actor.user_id).fetch_one(&mut hold).await.unwrap();
            assert_eq!(committed_role, "guest");
            assert!(!request.is_finished());
            eprintln!("source revoke-first committed before release: replay={replay} controlA={hold_pid} controlB={revoke_pid} product={product_pid} role={committed_role}");
            sqlx::query("COMMIT").execute(&mut hold).await.unwrap();
            let (status, denied) = request.await.unwrap();
            assert_eq!(status, StatusCode::NOT_FOUND, "revoked source must deny fresh/replay: {denied}");
            assert_eq!(counts(&run, source, document, task).await, source_before);
            assert_eq!(counts(&run, actor.workspace_id, document, task).await, destination_before);
        }
        run.finish().await.unwrap();
    }).await;
}

#[tokio::test]
async fn personal_transfer_operation_first_holds_source_owner_until_commit() {
    run_test("personal_transfer_operation_first_holds_source_owner_until_commit", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, selection) = setup_transfer(&mut run).await;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let body = command(addr, &actor, source, &selection).await;
        let request_id = Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap();
        let source_before = counts(&run, source, document, task).await;
        // Fixture-only request-guarded barrier after graph/event/activity writes
        // and before the receipt insertion. No product hook or definer bypass.
        let admin = admin_pool(&run.harness).await;
        sqlx::raw_sql(&format!(r#"CREATE FUNCTION fvoci.fixture_transfer_commit_barrier() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,fvoci AS $$ BEGIN
          IF NEW.request_id='{request_id}'::uuid THEN PERFORM pg_advisory_xact_lock(1465135717,13); END IF; RETURN NEW; END $$;
          CREATE TRIGGER fixture_transfer_commit_barrier BEFORE INSERT ON fvoci.personal_transfer_commands FOR EACH ROW EXECUTE FUNCTION fvoci.fixture_transfer_commit_barrier();"#))
            .execute(&admin).await.unwrap();
        let mut hold = scoped_observer(&run, source).await;
        let hold_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut hold).await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(1465135717,13)")
            .execute(&mut hold).await.unwrap();
        let token = actor.session_token.clone();
        let request_path = path(source);
        let payload = body.clone();
        let product = tokio::spawn(async move {
            session_call(addr, Method::POST, &request_path, &token, Some(payload)).await
        });
        let mut revoke = scoped_observer(&run, source).await;
        let revoke_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut revoke).await.unwrap();
        let product_pid = transfer_blocked_by(&mut revoke, hold_pid).await;
        assert_ne!(hold_pid, revoke_pid);
        assert_ne!(product_pid, hold_pid);
        assert_ne!(product_pid, revoke_pid);
        let query: String = sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE pid=$1")
            .bind(product_pid).fetch_one(&mut revoke).await.unwrap();
        assert!(query.contains("INSERT INTO fvoci.personal_transfer_commands"), "receipt-stage barrier: {query}");
        let actor_id = actor.user_id;
        let revocation = tokio::spawn(async move {
            let updated = sqlx::query("UPDATE fvoci.memberships SET role='guest' WHERE workspace_id=$1 AND user_id=$2")
                .bind(source).bind(actor_id).execute(&mut revoke).await.unwrap();
            assert_eq!(updated.rows_affected(), 1);
            // This independent connection can see the receipt only after the
            // publication commits, before its own revocation COMMIT.
            let published: (Uuid, Uuid) = sqlx::query_as("SELECT document_id,task_id FROM fvoci.personal_transfer_commands WHERE workspace_id=$1 AND request_id=$2")
                .bind(source).bind(request_id).fetch_one(&mut revoke).await.unwrap();
            assert_eq!(published, (document, task));
            sqlx::query("COMMIT").execute(&mut revoke).await.unwrap();
            eprintln!("operation-first revocation committed after visible publication: controlB={revoke_pid} receipt={request_id}");
        });
        let blocked_revoke = transfer_blocked_by(&mut hold, product_pid).await;
        assert_eq!(blocked_revoke, revoke_pid);
        let query: String = sqlx::query_scalar("SELECT query FROM pg_stat_activity WHERE pid=$1")
            .bind(revoke_pid).fetch_one(&mut hold).await.unwrap();
        assert!(query.contains("UPDATE fvoci.memberships SET role='guest'"));
        assert!(!product.is_finished());
        assert!(!revocation.is_finished());
        eprintln!("operation-first held authority before release: controlA={hold_pid} controlB={revoke_pid} product={product_pid}; B blocked by product, product blocked by A");
        sqlx::query("COMMIT").execute(&mut hold).await.unwrap();
        let (status, published) = product.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{published}");
        assert_eq!(published["documentId"], document.to_string());
        assert_eq!(published["taskId"], task.to_string());
        revocation.await.unwrap();
        sqlx::raw_sql("DROP TRIGGER fixture_transfer_commit_barrier ON fvoci.personal_transfer_commands;DROP FUNCTION fvoci.fixture_transfer_commit_barrier();")
            .execute(&admin).await.unwrap();
        admin.close().await;
        assert_eq!(counts(&run, source, document, task).await, (0,0,0,0,0,0,source_before.6+2));
        assert_eq!(counts(&run, actor.workspace_id, document, task).await, (1,1,1,1,1,1,2));
        let (status, _) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "later receipt replay must recheck revoked source owner");
        run.finish().await.unwrap();
    }).await;
}

#[tokio::test]
async fn personal_transfer_incoming_current_native_refs_refuse_move_copy_keeps_private_original() {
    run_test("personal_transfer_incoming_current_native_refs_refuse_move_copy_keeps_private_original",async {
        for (source_kind,target_kind,node_kind) in [(CollabKind::Document,"task","mention"),(CollabKind::Task,"document","embed"),(CollabKind::Document,"document","mention")] {
            let mut run=TestRun::new(TestDb::bootstrap().await);
            let(addr,actor,source,_,_,mut selection)=setup_transfer(&mut run).await;
            let personal=in_personal(&actor,source);
            let document=Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
            let task=Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
            let referrer=if source_kind==CollabKind::Document {
                create_wiki(&personal,"절대 공개하지 않을 참조 문서").await
            } else {
                let(status,pair)=session_call(addr,Method::POST,&format!("/api/v1/workspaces/{source}/personal-input"),&actor.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"task","title":"비공개 참조 작업"}))).await;
                assert_eq!(status,StatusCode::CREATED,"{pair}");Uuid::parse_str(pair["taskId"].as_str().unwrap()).unwrap()
            };
            let target=if target_kind=="task" {task} else {document};
            let mut attrs=json!({"entity":target_kind,"label":"비공개 원본 문맥"});
            attrs[if node_kind=="mention" {"id"} else {"ref"}]=json!(target);
            let content=if node_kind=="mention" {
                json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":Uuid::now_v7()},"content":[{"type":node_kind,"attrs":attrs}]}]})
            } else {json!({"type":"doc","content":[{"type":node_kind,"attrs":attrs}]})};
            // Genuine native writer and matched persist ACK, then a fixture-
            // only stale derived-cache control. No JSON-only fake reference.
            let seed=seed_update(&run,content).await;
            let key=CollabRoomName {workspace_id:source,kind:source_kind,resource_id:referrer}.routing_key();
            apply_and_persist(addr,&actor.session_token,&key,91,&seed).await;
            let table=if source_kind==CollabKind::Document {"documents"} else {"tasks"};
            let admin=admin_pool(&run.harness).await;
            sqlx::raw_sql(&format!(r#"CREATE FUNCTION fvoci.fixture_transfer_stale_projection() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,fvoci AS $$ BEGIN IF NEW.id='{referrer}'::uuid THEN NEW.content_json='{{"type":"doc","content":[{{"type":"paragraph"}}]}}'::jsonb; END IF;RETURN NEW;END $$; CREATE TRIGGER fixture_transfer_stale_projection BEFORE UPDATE OF content_json ON fvoci.{table} FOR EACH ROW EXECUTE FUNCTION fvoci.fixture_transfer_stale_projection();"#)).execute(&admin).await.unwrap();
            let mut conn=scoped_observer(&run,source).await;
            sqlx::query(&format!("UPDATE fvoci.{table} SET content_json=$2 WHERE workspace_id=$1 AND id=$3"))
                .bind(source).bind(fvoci_server::db::documents::empty_document_json()).bind(referrer).execute(&mut conn).await.unwrap();
            let cached:Value=sqlx::query_scalar(&format!("SELECT content_json FROM fvoci.{table} WHERE workspace_id=$1 AND id=$2"))
                .bind(source).bind(referrer).fetch_one(&mut conn).await.unwrap();
            assert!(fvoci_server::collab::derived_body::extract_internal_refs(&cached).is_empty());
            sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
            let before=transfer_graph(&run,source).await;
            let target_before=transfer_graph(&run,actor.workspace_id).await;
            let(status,error)=session_call(addr,Method::POST,&format!("{}/preview",path(source)),&actor.session_token,Some(selection.clone())).await;
            assert_eq!(status,StatusCode::CONFLICT,"native incoming {source_kind:?}->{target_kind} must refuse: {error}");
            assert_eq!(error["code"],"personal_transfer_incomplete");
            assert_eq!(error["title"],"incoming private reference mapping");
            assert_eq!(error["params"]["code"],"incoming_reference");
            assert_eq!(transfer_graph(&run,source).await,before);
            assert_eq!(transfer_graph(&run,actor.workspace_id).await,target_before);
            // COPY preserves the original target and private referrer; it must
            // not inherit MOVE's incoming-edge refusal or publish the referrer.
            selection["action"]=json!("copy");
            let body=command(addr,&actor,source,&selection).await;
            let(status,copied)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(body.clone())).await;
            assert_eq!(status,StatusCode::OK,"{copied}");
            assert_ne!(copied["documentId"],document.to_string());assert_ne!(copied["taskId"],task.to_string());
            assert_eq!(transfer_graph(&run,source).await,before,"COPY leaves original native/history/reference/ACL/outbox graph unchanged");
            let outsider=create_user_session(&run.harness,actor.workspace_id,WorkspaceRole::Member).await;
            let source_path=if source_kind==CollabKind::Document {document_api(source,referrer,"")} else {task_path(&personal,referrer,"")};
            let(status,_)=session_call(addr,Method::GET,&source_path,&outsider.session_token,None).await;
            assert_eq!(status,StatusCode::NOT_FOUND);
            sqlx::raw_sql(&format!("DROP TRIGGER fixture_transfer_stale_projection ON fvoci.{table};DROP FUNCTION fvoci.fixture_transfer_stale_projection();")).execute(&admin).await.unwrap();admin.close().await;
            eprintln!("current native incoming edge refused with cached JSON empty; COPY retained private source: {source_kind:?}->{target_kind} {node_kind}");
            run.finish().await.unwrap();
        }
    }).await;
}

#[tokio::test]
async fn personal_transfer_copy_current_native_content_has_new_blocks_and_preserves_history() {
    run_test("personal_transfer_copy_current_native_content_has_new_blocks_and_preserves_history",async {
        let mut run=TestRun::new(TestDb::bootstrap().await);
        let(addr,actor,source,_,_,mut selection)=setup_transfer(&mut run).await;
        let document=Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task=Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let document_block=Uuid::now_v7();let task_block=Uuid::now_v7();
        let doc_json=json!({"type":"doc","content":[para(&document_block.to_string(),"문서 현재 본문 🧑‍💻")]});
        let task_json=json!({"type":"doc","content":[para(&task_block.to_string(),"작업 현재 본문 日本語")]});
        let(status,metadata)=session_call(addr,Method::PUT,&document_api(source,document,"/body"),&actor.session_token,Some(json!({"contentJson":doc_json}))).await;
        assert_eq!(status,StatusCode::OK,"{metadata}");
        let seed=seed_update(&run,task_json).await;
        apply_and_persist(addr,&actor.session_token,&task_key(source,task),93,&seed).await;
        // A body write is not a revision: create the original histories through
        // the real revision endpoints (40eff85 showed none existed before).
        let personal=in_personal(&actor,source);
        for revisions in [document_api(source,document,"/revisions"),task_path(&personal,task,"/revisions")] {
            let(status,created)=session_call(addr,Method::POST,&revisions,&actor.session_token,None).await;
            assert_eq!(status,StatusCode::CREATED,"{revisions}: {created}");
        }
        selection_versions(&run,source,&mut selection).await;
        selection["action"]=json!("copy");
        let before=transfer_graph(&run,source).await;
        assert!(!before["document_states"].as_array().unwrap().is_empty());
        assert!(!before["task_states"].as_array().unwrap().is_empty());
        assert!(before["revisions"].as_array().unwrap().iter().any(|row|row["target_id"]==document.to_string()));
        assert!(before["revisions"].as_array().unwrap().iter().any(|row|row["target_id"]==task.to_string()));
        let body=command(addr,&actor,source,&selection).await;
        let (status,preview)=session_call(addr,Method::POST,&format!("{}/preview",path(source)),&actor.session_token,Some(selection.clone())).await;
        assert_eq!(status,StatusCode::OK,"{preview}");
        let observed=before["revisions"].as_array().unwrap().iter().filter(|row|row["target_id"]==document.to_string()||row["target_id"]==task.to_string()).count();
        assert!(preview["dispositions"].as_array().unwrap().contains(&json!({"item":"history","outcome":"retained_private","count":observed})),"{preview}");
        assert!(preview["dispositions"].as_array().unwrap().contains(&json!({"item":"activity","outcome":"not_included","count":preview["activityCount"]})),"{preview}");
        assert_eq!(transfer_graph(&run,source).await,before,"preview seeds no source state and changes no history");
        let client=reqwest::Client::new();
        let response=client.post(format!("http://{addr}{}",path(source))).header("origin",PUBLIC_ORIGIN).header("cookie",format!("fvoci_session={}",actor.session_token)).json(&body).send().await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);drop(response);
        let(status,copied)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(body.clone())).await;
        assert_eq!(status,StatusCode::OK,"{copied}");assert_eq!(copied["replayed"],true);
        let copied_doc=Uuid::parse_str(copied["documentId"].as_str().unwrap()).unwrap();let copied_task=Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
        assert_ne!(copied_doc,document);assert_ne!(copied_task,task);
        assert_eq!(transfer_graph(&run,source).await,before,"original native/history/ACL/outbox are byte-equivalent");
        let mut conn=scoped_observer(&run,actor.workspace_id).await;
        let rows:(Value,Value,Vec<u8>,Vec<u8>,i64,i64,i64)=sqlx::query_as("SELECT d.content_json,t.content_json,ds.state,ts.state,(SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_id=ANY(ARRAY[$2,$3])),(SELECT count(*) FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2),(SELECT count(*) FROM fvoci.task_collab_op_receipts WHERE workspace_id=$1 AND task_id=$3) FROM fvoci.documents d JOIN fvoci.tasks t ON t.workspace_id=d.workspace_id JOIN fvoci.document_states ds ON ds.workspace_id=d.workspace_id AND ds.document_id=d.id JOIN fvoci.task_states ts ON ts.workspace_id=t.workspace_id AND ts.task_id=t.id WHERE d.workspace_id=$1 AND d.id=$2 AND t.id=$3")
            .bind(actor.workspace_id).bind(copied_doc).bind(copied_task).fetch_one(&mut conn).await.unwrap();
        assert_eq!(rows.0["content"][0]["content"][0]["text"],"문서 현재 본문 🧑‍💻");
        assert_eq!(rows.1["content"][0]["content"][0]["text"],"작업 현재 본문 日本語");
        assert_ne!(rows.0["content"][0]["attrs"]["id"],document_block.to_string());
        assert_ne!(rows.1["content"][0]["attrs"]["id"],task_block.to_string());
        assert_eq!((rows.4,rows.5,rows.6),(0,0,0),"current-content COPY creates no cloned history/old command receipts");
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
        // Independent native reads of committed new seeds, not stored JSON.
        let hub=run.hub();
        for (snapshot,expected) in [(rows.2,rows.0),(rows.3,rows.1)] {
            let projected=fvoci_server::collab::revision::project_persisted_offline(hub.engine_bin(),hub.limits(),snapshot,vec![]).unwrap();
            assert_eq!(projected,expected);
        }
        let destination_before=transfer_graph(&run,actor.workspace_id).await;
        let(status,again)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(body.clone())).await;
        assert_eq!(status,StatusCode::OK);assert_eq!(again["taskId"],copied["taskId"]);
        assert_eq!(transfer_graph(&run,actor.workspace_id).await,destination_before);
        let mut changed=body;changed["selection"]["action"]=json!("move");
        let(status,error)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(changed)).await;
        assert_eq!(status,StatusCode::CONFLICT);
        assert_eq!(error["params"]["code"],"command_changed");
        assert_eq!(transfer_graph(&run,source).await,before);
        run.finish().await.unwrap();
    }).await;
}

/// COPY must map a non-null origin anchor onto the copied block's NEW id,
/// for a UUID written in another case and for a legacy non-UUID block id.
/// Both pairs are created through the real body PUT and block-capture paths.
#[tokio::test]
async fn personal_transfer_copy_maps_uuid_and_legacy_origin_anchors_to_new_blocks() {
    run_test("personal_transfer_copy_maps_uuid_and_legacy_origin_anchors_to_new_blocks",async {
        for legacy in [false,true] {
            let mut run=TestRun::new(TestDb::bootstrap().await);
            let(addr,actor,source,_,_,mut selection)=setup_transfer(&mut run).await;
            let(status,note)=session_call(addr,Method::POST,&format!("/api/v1/workspaces/{source}/personal-input"),&actor.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"note","title":"원본 한글 🧑‍💻"}))).await;
            assert_eq!(status,StatusCode::CREATED,"{note}");
            let document=Uuid::parse_str(note["documentId"].as_str().unwrap()).unwrap();
            let block=if legacy {"legacy-anchor-7".to_string()} else {Uuid::now_v7().to_string()};
            let anchor=if legacy {block.clone()} else {block.to_uppercase()};
            let body=json!({"type":"doc","content":[para("before-block","앞 문단"),para(&block,"출처 블록 日本語 🙂")]});
            let(status,saved)=session_call(addr,Method::PUT,&document_api(source,document,"/body"),&actor.session_token,Some(json!({"contentJson":body}))).await;
            assert_eq!(status,StatusCode::OK,"{saved}");
            let(status,pair)=session_call(addr,Method::POST,&format!("/api/v1/workspaces/{source}/personal-input"),&actor.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"task","title":"블록에서 만든 작업","source":{"documentId":document,"anchor":anchor}}))).await;
            assert_eq!(status,StatusCode::CREATED,"{pair}");
            let task=Uuid::parse_str(pair["taskId"].as_str().unwrap()).unwrap();
            selection["documentId"]=json!(document);selection["taskId"]=json!(task);selection["action"]=json!("copy");
            selection_versions(&run,source,&mut selection).await;
            let before=transfer_graph(&run,source).await;
            let original_origin=before["task_origins"].as_array().unwrap().iter().find(|row|row["task_id"]==task.to_string()).unwrap().clone();
            assert_eq!(original_origin["anchor"],anchor,"the private origin keeps its exact anchor");
            let command_body=command(addr,&actor,source,&selection).await;
            let(status,copied)=session_call(addr,Method::POST,&path(source),&actor.session_token,Some(command_body)).await;
            assert_eq!(status,StatusCode::OK,"{copied}");
            let copied_doc=Uuid::parse_str(copied["documentId"].as_str().unwrap()).unwrap();
            let copied_task=Uuid::parse_str(copied["taskId"].as_str().unwrap()).unwrap();
            let mut conn=scoped_observer(&run,actor.workspace_id).await;
            let(content,copied_anchor):(Value,Option<String>)=sqlx::query_as("SELECT d.content_json,o.anchor FROM fvoci.documents d JOIN fvoci.task_origins o ON o.workspace_id=d.workspace_id AND o.document_id=d.id WHERE d.workspace_id=$1 AND d.id=$2 AND o.task_id=$3")
                .bind(actor.workspace_id).bind(copied_doc).bind(copied_task).fetch_one(&mut conn).await.unwrap();
            sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
            let blocks=content["content"].as_array().unwrap();
            assert_eq!(blocks[1]["content"][0]["text"],"출처 블록 日本語 🙂");
            let new_block=blocks[1]["attrs"]["id"].as_str().unwrap().to_string();
            assert!(Uuid::parse_str(&new_block).is_ok());
            assert_ne!(new_block.to_lowercase(),block.to_lowercase(),"the copy never reuses the private block id");
            assert_ne!(blocks[0]["attrs"]["id"],"before-block");
            assert_eq!(copied_anchor.as_deref(),Some(new_block.as_str()),"origin anchor follows its copied block (legacy={legacy})");
            assert_eq!(transfer_graph(&run,source).await,before,"COPY leaves the private pair and its anchor unchanged");
            run.finish().await.unwrap();
        }
    }).await;
}

/// Committed collab rows of one document. State row (bytes, cutoff, tail,
/// updated_at), tail updates and events are read by the restricted app role.
/// audit_log is readable only in system context, so its rows come from the
/// harness preparation connection as an observer only, never authority.
async fn collab_rows(run: &TestRun, ws: Uuid, document: Uuid) -> Value {
    let mut conn = scoped_observer(run, ws).await;
    let mut rows: Value = sqlx::query_scalar(
        "SELECT json_build_object(
          'state',(SELECT to_jsonb(s) FROM fvoci.document_states s WHERE workspace_id=$1 AND document_id=$2),
          'updates',(SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY u.seq),'[]') FROM fvoci.document_collab_updates u WHERE workspace_id=$1 AND document_id=$2),
          'events',(SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY e.xact,e.seq),'[]') FROM fvoci.events e WHERE workspace_id=$1 AND target_id=$2))::jsonb",
    )
    .bind(ws)
    .bind(document)
    .fetch_one(&mut conn)
    .await
    .unwrap();
    sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
    let admin = admin_pool(&run.harness).await;
    let audit: Value = sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]') FROM fvoci.audit_log a WHERE workspace_id=$1 AND target_id=$2")
        .bind(ws)
        .bind(document)
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    rows["audit"] = audit;
    rows
}

async fn persist_once(
    ws_conn: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    key: &str,
) {
    let request_id = Uuid::now_v7();
    ws_conn
        .send(Message::Binary(
            stateless_frame(key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_stateless_exact(
            ws_conn,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await,
        "persist ack"
    );
}

/// Preview/Cancel's host save must not write when nothing changed: saving the
/// committed snapshot again is a no-op, a real update still compacts, and
/// writer/cutoff/session checks still reject before any no-op.
#[tokio::test]
async fn personal_transfer_repeated_save_of_unchanged_snapshot_is_a_noop() {
    run_test("personal_transfer_repeated_save_of_unchanged_snapshot_is_a_noop", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, selection) = setup_transfer(&mut run).await;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let body = json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"저장 반복 확인 🙂")]});
        let (status, saved) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson":body}))).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let key = CollabRoomName { workspace_id: source, kind: CollabKind::Document, resource_id: document }.routing_key();
        let mut ws_conn = connect_member(addr, &actor.session_token).await;
        auth_and_join(&mut ws_conn, &key, 77).await;
        complete_sync_handshake(&mut ws_conn, &key).await;
        persist_once(&mut ws_conn, &key).await;
        let first = collab_rows(&run, source, document).await;
        assert!(!first["state"].is_null(), "a committed native state exists");
        assert!(!first["audit"].as_array().unwrap().is_empty(), "audit observer sees rows");
        persist_once(&mut ws_conn, &key).await;
        let second = collab_rows(&run, source, document).await;
        assert_eq!(second, first, "repeated save of the unchanged snapshot writes nothing");

        // Direct calls with the very same stored snapshot still reject.
        let state = &first["state"];
        let stored_hex = state["state"].as_str().unwrap().trim_start_matches("\\x");
        let stored: Vec<u8> = (0..stored_hex.len()).step_by(2).map(|i| u8::from_str_radix(&stored_hex[i..i + 2], 16).unwrap()).collect();
        let generation = state["writer_generation"].as_i64().unwrap();
        let cutoff = state["snapshot_cutoff_seq"].as_i64().unwrap();
        let tail = state["tail_seq"].as_i64().unwrap();
        assert_eq!(cutoff, tail, "fully compacted before the checks");
        let input = |session_id: Uuid, writer_generation: i64, expected_tail_seq: i64| fvoci_server::db::collab::CompactCollabInput {
            workspace_id: source, actor_user_id: actor.user_id, session_id, document_id: document,
            writer_generation, cutoff_seq: cutoff, expected_tail_seq, new_snapshot: &stored, client_ip: None,
        };
        use fvoci_server::db::collab::{compact_collab_snapshot_kind, CollabDbError};
        let stale_writer = compact_collab_snapshot_kind(&actor.pool, CollabKind::Document, input(actor.session_id, generation + 1, tail)).await.unwrap();
        assert!(matches!(stale_writer, Err(CollabDbError::StaleWriter)));
        let stale_cutoff = compact_collab_snapshot_kind(&actor.pool, CollabKind::Document, input(actor.session_id, generation, tail + 1)).await.unwrap();
        assert!(matches!(stale_cutoff, Err(CollabDbError::StaleCutoff)));
        let foreign_session = compact_collab_snapshot_kind(&actor.pool, CollabKind::Document, input(Uuid::now_v7(), generation, tail)).await.unwrap();
        assert!(foreign_session.is_err(), "inactive session rejects even a no-op: {foreign_session:?}");
        assert_eq!(collab_rows(&run, source, document).await, first, "rejections change nothing");

        // A genuine update still compacts with its event and audit.
        let update = seed_update(&run, json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"새 변경 日本語")]})).await;
        ws_conn.send(Message::Binary(sync_update_frame(&key, &update).into())).await.unwrap();
        assert!(wait_for_sync_applied(&mut ws_conn, Duration::from_secs(8)).await, "update must apply");
        persist_once(&mut ws_conn, &key).await;
        let third = collab_rows(&run, source, document).await;
        assert_ne!(third["state"]["state"], first["state"]["state"]);
        let compacted = |rows: &Value| rows["events"].as_array().unwrap().iter().filter(|e| e["verb"] == "document.collab_snapshot_compacted").count();
        assert_eq!(compacted(&third), compacted(&first) + 1);
        assert!(third["audit"].as_array().unwrap().len() > first["audit"].as_array().unwrap().len());
        // Exact revoked EXISTING session (not a random ID): close the room
        // connection (only an explicit persist compacts), settle the committed
        // rows, revoke the actor's real live session through the real logout
        // route, then offer the very same committed bytes and cutoff.
        let _ = ws_conn.close(None).await;
        let settled = collab_rows(&run, source, document).await;
        let hex = settled["state"]["state"].as_str().unwrap().trim_start_matches("\\x").to_string();
        let latest: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let settled_generation = settled["state"]["writer_generation"].as_i64().unwrap();
        let settled_tail = settled["state"]["tail_seq"].as_i64().unwrap();
        assert_eq!(settled["state"]["snapshot_cutoff_seq"].as_i64().unwrap(), settled_tail, "a same-cutoff no-op candidate");
        let (status, logout) = session_call(addr, Method::POST, "/api/v1/auth/logout", &actor.session_token, None).await;
        assert!(status.is_success(), "{status} {logout}");
        let admin = admin_pool(&run.harness).await;
        let (exists, revoked): (bool, bool) = sqlx::query_as("SELECT true, revoked_at IS NOT NULL FROM fvoci.sessions WHERE id=$1")
            .bind(actor.session_id).fetch_one(&admin).await.unwrap();
        admin.close().await;
        assert_eq!((exists, revoked), (true, true), "the actor's real session exists and is revoked");
        let refused = compact_collab_snapshot_kind(&actor.pool, CollabKind::Document, fvoci_server::db::collab::CompactCollabInput {
            workspace_id: source, actor_user_id: actor.user_id, session_id: actor.session_id, document_id: document,
            writer_generation: settled_generation, cutoff_seq: settled_tail, expected_tail_seq: settled_tail,
            new_snapshot: &latest, client_ip: None,
        }).await.unwrap();
        assert!(matches!(refused, Err(CollabDbError::Forbidden)), "revoked existing session refuses even a same-bytes no-op: {refused:?}");
        assert_eq!(collab_rows(&run, source, document).await, settled, "refusal changes nothing");
        run.finish().await.unwrap();
    }).await;
}

#[path = "personal_transfer_zotero.rs"]
mod zotero;

#[path = "personal_transfer_files.rs"]
mod files;

#[path = "personal_transfer_current.rs"]
mod current;

fn document_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

/// A personal pair with genuine native document/task state, receipts and
/// one real revision each (native writer + revision endpoints).
async fn native_pair(
    addr: SocketAddr,
    run: &TestRun,
    actor: &SessionFixture,
    source: Uuid,
    selection: &mut Value,
) -> (Uuid, Uuid) {
    let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
    let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
    let doc_seed = seed_update(
        run,
        json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"이동할 문서 본문 🧑‍💻")]}),
    )
    .await;
    apply_and_persist(
        addr,
        &actor.session_token,
        &document_key(source, document),
        96,
        &doc_seed,
    )
    .await;
    let task_seed = seed_update(run, json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"이동할 작업 본문 日本語")]})).await;
    apply_and_persist(
        addr,
        &actor.session_token,
        &task_key(source, task),
        97,
        &task_seed,
    )
    .await;
    let personal = in_personal(actor, source);
    for revisions in [
        document_api(source, document, "/revisions"),
        task_path(&personal, task, "/revisions"),
    ] {
        let (status, created) =
            session_call(addr, Method::POST, &revisions, &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{revisions}: {created}");
    }
    selection_versions(run, source, selection).await;
    (document, task)
}

/// Rows of one table that belong to `id` in a committed transfer graph,
/// optionally re-labelled with another workspace, in a stable order.
/// A restore under the adopted W4 contract: read the route's restore-preview,
/// then restore against its current tail.
async fn previewed_restore(addr: SocketAddr, restore: &str, token: &str) -> (StatusCode, Value) {
    let preview_route = format!("{restore}-preview");
    let (status, preview) = session_call(addr, Method::GET, &preview_route, token, None).await;
    assert_eq!(status, StatusCode::OK, "{preview_route}: {preview}");
    session_call(
        addr,
        Method::POST,
        restore,
        token,
        Some(
            json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": preview["currentTailSeq"]}),
        ),
    )
    .await
}

fn rows_of(
    graph: &Value,
    table: &str,
    column: &str,
    id: Uuid,
    workspace: Option<Uuid>,
) -> Vec<Value> {
    let mut rows: Vec<Value> = graph[table]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row[column] == id.to_string())
        .cloned()
        .map(|mut row| {
            if let Some(ws) = workspace {
                row["workspace_id"] = json!(ws);
            }
            row
        })
        .collect();
    rows.sort_by_key(|row| row.to_string());
    rows
}

const NATIVE_TABLES: [(&str, &str, bool); 6] = [
    ("document_states", "document_id", true),
    ("document_collab_updates", "document_id", false),
    ("document_collab_op_receipts", "document_id", true),
    ("task_states", "task_id", false),
    ("task_collab_updates", "task_id", false),
    ("task_collab_op_receipts", "task_id", false),
];

#[tokio::test]
async fn personal_transfer_move_carries_native_history_with_same_ids() {
    run_test("personal_transfer_move_carries_native_history_with_same_ids", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
        let (document, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
        let team = actor.workspace_id;
        let before = transfer_graph(&run, source).await;
        for (table, column, required) in NATIVE_TABLES {
            let id = if column == "document_id" { document } else { task };
            assert!(!required || !rows_of(&before, table, column, id, None).is_empty(), "{table} fixture");
        }
        assert!(!rows_of(&before, "task_states", "task_id", task, None).is_empty());
        assert!(!rows_of(&before, "task_collab_op_receipts", "task_id", task, None).is_empty());
        let revisions = rows_of(&before, "revisions", "target_id", document, None).len()
            + rows_of(&before, "revisions", "target_id", task, None).len();
        assert_eq!(revisions, 2);

        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        assert!(preview["dispositions"].as_array().unwrap().contains(&json!({"item":"history","outcome":"moved","count":revisions})), "{preview}");
        assert_eq!(transfer_graph(&run, source).await, before, "preview has no effects");
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        assert_eq!(moved["documentId"], document.to_string());
        assert_eq!(moved["taskId"], task.to_string());

        // Every retained native row moved byte-identical with the same IDs,
        // only the workspace changed; none is left in the private source.
        let after_source = transfer_graph(&run, source).await;
        let after_team = transfer_graph(&run, team).await;
        let mut tables: Vec<(&str, &str, Uuid)> = NATIVE_TABLES
            .iter()
            .map(|(table, column, _)| (*table, *column, if *column == "document_id" { document } else { task }))
            .collect();
        tables.push(("revisions", "target_id", document));
        tables.push(("revisions", "target_id", task));
        for (table, column, id) in tables {
            assert_eq!(rows_of(&after_team, table, column, id, None), rows_of(&before, table, column, id, Some(team)), "{table} {id}");
            assert!(rows_of(&after_source, table, column, id, None).is_empty(), "{table} left in source");
        }

        // Independent native read of the moved committed state and tail.
        let mut conn = scoped_observer(&run, team).await;
        let (state, body_json): (Vec<u8>, Value) = sqlx::query_as("SELECT s.state, d.content_json FROM fvoci.document_states s JOIN fvoci.documents d ON d.workspace_id=s.workspace_id AND d.id=s.document_id WHERE s.workspace_id=$1 AND s.document_id=$2")
            .bind(team).bind(document).fetch_one(&mut conn).await.unwrap();
        let tail: Vec<Vec<u8>> = sqlx::query_scalar("SELECT payload FROM fvoci.document_collab_updates WHERE workspace_id=$1 AND document_id=$2 ORDER BY seq")
            .bind(team).bind(document).fetch_all(&mut conn).await.unwrap();
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
        let hub = run.hub();
        let projected = fvoci_server::collab::revision::project_persisted_offline(hub.engine_bin(), hub.limits(), state, tail).unwrap();
        assert_eq!(projected, body_json);

        let (status, _) = session_call(addr, Method::GET, &document_api(source, document, ""), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "the private route is closed");
        let destination_before = transfer_graph(&run, team).await;
        let (status, again) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{again}");
        assert_eq!(again["replayed"], true);
        assert_eq!(again["documentId"], document.to_string());
        assert_eq!(transfer_graph(&run, team).await, destination_before, "replay moves nothing twice");

        // The moved history keeps working for a distinct team member: their
        // edit appends a receipt in the team room, and restoring a moved
        // revision brings back the original body.
        let peer = create_user_session(&run.harness, team, WorkspaceRole::Member).await;
        add_project_member(&run.harness, team, project, peer.user_id, "member").await;
        let task_revision = rows_of(&destination_before, "revisions", "target_id", task, None)[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let receipts = rows_of(&destination_before, "task_collab_op_receipts", "task_id", task, None).len();
        let edit = seed_update(&run, json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"동료 편집 이후")]})).await;
        apply_and_persist(addr, &peer.session_token, &task_key(team, task), 98, &edit).await;
        let after_edit = transfer_graph(&run, team).await;
        let peer_receipts = rows_of(&after_edit, "task_collab_op_receipts", "task_id", task, None);
        assert_eq!(peer_receipts.len(), receipts + 1);
        assert!(peer_receipts.iter().any(|row| row["actor_user_id"] == peer.user_id.to_string()));
        let admin = admin_pool(&run.harness).await;
        wait_task_content_contains(&admin, team, task, "동료 편집 이후").await;
        let (status, restored) = previewed_restore(
            addr,
            &task_path(&peer, task, &format!("/revisions/{task_revision}/restore")),
            &peer.session_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);
        let (content, _, _) = wait_task_content_contains(&admin, team, task, "이동할 작업 본문").await;
        assert!(!content.to_string().contains("동료 편집 이후"), "{content}");
        admin.close().await;
        run.finish().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn personal_transfer_move_preview_is_stale_after_new_native_history() {
    run_test("personal_transfer_move_preview_is_stale_after_new_native_history", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        let (document, _) = native_pair(addr, &run, &actor, source, &mut selection).await;
        // Fixture: the latest document revision becomes automatic, as a
        // session revision of the same snapshot would be.
        let admin = admin_pool(&run.harness).await;
        let revision: Uuid = sqlx::query_scalar("UPDATE fvoci.revisions SET reason='session' WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2 RETURNING id")
            .bind(source).bind(document).fetch_one(&admin).await.unwrap();
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        // A manual revision of the same snapshot upgrades that revision in
        // place (product path): history metadata changes, nothing else does.
        let documents_before: Value = sqlx::query_scalar("SELECT to_jsonb(d) FROM fvoci.documents d WHERE workspace_id=$1 AND id=$2")
            .bind(source).bind(document).fetch_one(&admin).await.unwrap();
        let (status, created) = session_call(addr, Method::POST, &document_api(source, document, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["id"], revision.to_string(), "upgraded in place");
        let (reason, count): (String, i64) = sqlx::query_as("SELECT (SELECT reason FROM fvoci.revisions WHERE id=$3), (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_id=$2)")
            .bind(source).bind(document).bind(revision).fetch_one(&admin).await.unwrap();
        assert_eq!((reason.as_str(), count), ("manual", 1), "the history really changed");
        let documents_after: Value = sqlx::query_scalar("SELECT to_jsonb(d) FROM fvoci.documents d WHERE workspace_id=$1 AND id=$2")
            .bind(source).bind(document).fetch_one(&admin).await.unwrap();
        assert_eq!(documents_after, documents_before, "body/version/updated_at unchanged");
        let mut current = selection.clone();
        selection_versions(&run, source, &mut current).await;
        assert_eq!(current, selection, "no document/task version changed");
        admin.close().await;
        let before = transfer_graph(&run, source).await;
        let target_before = transfer_graph(&run, actor.workspace_id).await;
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "preview_stale");
        assert_eq!(transfer_graph(&run, source).await, before);
        assert_eq!(transfer_graph(&run, actor.workspace_id).await, target_before);
        run.finish().await.unwrap();
    })
    .await;
}

/// Without the TEMP privilege the native copy cannot be staged: MOVE
/// refuses at preview and at commit before any effect (no partial move).
#[tokio::test]
async fn personal_transfer_move_without_staging_privilege_refuses_before_effects() {
    run_test(
        "personal_transfer_move_without_staging_privilege_refuses_before_effects",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
            native_pair(addr, &run, &actor, source, &mut selection).await;
            let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
            assert_eq!(status, StatusCode::OK, "{preview}");
            let admin = admin_pool(&run.harness).await;
            sqlx::query(&format!("REVOKE TEMPORARY ON DATABASE \"{}\" FROM PUBLIC", run.harness.db_name()))
                .execute(&admin)
                .await
                .unwrap();
            let mut conn = scoped_observer(&run, source).await;
            let temp: bool = sqlx::query_scalar("SELECT has_database_privilege(current_database(), 'TEMP')")
                .fetch_one(&mut conn)
                .await
                .unwrap();
            sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
            assert!(!temp, "fixture: the restricted role lost TEMP");
            let before = transfer_graph(&run, source).await;
            let target_before = transfer_graph(&run, actor.workspace_id).await;
            let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["params"]["code"], "native_history");
            let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
            let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
            assert_eq!(status, StatusCode::CONFLICT, "{error}");
            assert_eq!(error["params"]["code"], "native_history");
            assert_eq!(transfer_graph(&run, source).await, before);
            assert_eq!(transfer_graph(&run, actor.workspace_id).await, target_before);
            admin.close().await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

/// Joins `key` and reports the server's answer: authenticated, or the close
/// reason (a refused or unavailable join) — never panics on a refusal.
async fn join_reply(
    addr: SocketAddr,
    token: String,
    key: String,
    client: u32,
) -> Result<(), String> {
    let mut ws = connect_member(addr, &token).await;
    support::send_auth_token(&mut ws, &key, client).await;
    let reply = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .map_err(|_| "no reply".to_string())?;
    match reply {
        Some(Ok(Message::Binary(bytes))) => match fvoci_server::collab::wire::decode(&bytes) {
            Ok(WireFrame::Document {
                message: DocumentMessage::Auth(AuthMessage::Authenticated { .. }),
                ..
            }) => Ok(()),
            other => Err(format!("{other:?}")),
        },
        Some(Ok(Message::Close(frame))) => Err(format!("{frame:?}")),
        other => Err(format!("{other:?}")),
    }
}

/// A genuine native write that is applied and durable but not persisted:
/// it stays in the retained tail. The socket stays open (live room).
async fn apply_without_persist(
    addr: SocketAddr,
    token: &str,
    key: &str,
    client: u32,
    update: &[u8],
) -> Ws {
    let mut ws = connect_member(addr, token).await;
    auth_and_join(&mut ws, key, client).await;
    complete_sync_handshake(&mut ws, key).await;
    ws.send(Message::Binary(sync_update_frame(key, update).into()))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await,
        "update must apply"
    );
    ws
}

/// A distinct permitted team member edits the moved task in the team room.
async fn peer_edits_moved_task(
    addr: SocketAddr,
    run: &TestRun,
    team: Uuid,
    project: Uuid,
    task: Uuid,
) -> SessionFixture {
    let peer = create_user_session(&run.harness, team, WorkspaceRole::Member).await;
    add_project_member(&run.harness, team, project, peer.user_id, "member").await;
    let edit = seed_update(
        run,
        json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"이동 직후 동료 편집")]}),
    )
    .await;
    apply_and_persist(addr, &peer.session_token, &task_key(team, task), 231, &edit).await;
    peer
}

/// Starts a MOVE of the prepared native pair in a task (the response is
/// observed later, so a test can tell whether it answered too early).
fn spawn_move(
    addr: SocketAddr,
    actor: &SessionFixture,
    source: Uuid,
    body: Value,
) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    let token = actor.session_token.clone();
    tokio::spawn(async move {
        session_call(addr, Method::POST, &path(source), &token, Some(body)).await
    })
}

async fn move_body(
    addr: SocketAddr,
    actor: &SessionFixture,
    source: Uuid,
    selection: &Value,
) -> Value {
    let (status, preview) = session_call(
        addr,
        Method::POST,
        &format!("{}/preview", path(source)),
        &actor.session_token,
        Some(selection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true})
}

/// Until the MOVE response: any answer while `held` (a hook keeping the
/// source room's resource guard) is a premature success; the test proceeds
/// once the retirement is observed waiting on that room.
async fn assert_move_waits(
    hub: &std::sync::Arc<fvoci_server::collab::hub::CollabHub>,
    key: fvoci_server::collab::room::RoomKey,
    mover: &tokio::task::JoinHandle<(StatusCode, Value)>,
    held: &str,
) {
    loop {
        assert!(
            !mover.is_finished(),
            "MOVE answered while {held} still held the source room guard"
        );
        if hub.room_waiter_count(key).await > 0 {
            return;
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn personal_transfer_move_waits_for_a_starting_source_room() {
    run_test(
        "personal_transfer_move_waits_for_a_starting_source_room",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
            let (_, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
            let hub = run.hub();
            let key = fvoci_server::collab::room::RoomKey::task(source, task);
            hub.force_room_idle_eligible(key).await;
            assert!(
                hub.execute_idle_evict_if_eligible(key).await,
                "fixture: the idle source room is evicted"
            );
            // A source join (admitted before the MOVE) starts the room; its starter
            // has taken the resource guard and is held before publishing Live.
            let block = fvoci_server::collab::room::arm_spawn_room_block(task).await;
            let source_join = tokio::spawn(join_reply(
                addr,
                actor.session_token.clone(),
                task_key(source, task),
                211,
            ));
            while !fvoci_server::collab::room::spawn_room_block_reached(task).await {
                tokio::task::yield_now().await;
            }
            let body = move_body(addr, &actor, source, &selection).await;
            let mover = spawn_move(addr, &actor, source, body);
            assert_move_waits(&hub, key, &mover, "a starting source room").await;
            let _ = block.send(());
            let (status, moved) = mover.await.unwrap();
            assert_eq!(status, StatusCode::OK, "{moved}");
            assert!(
                source_join.await.unwrap().is_err(),
                "the old scope join is refused"
            );
            peer_edits_moved_task(addr, &run, actor.workspace_id, project, task).await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_move_waits_for_a_closing_source_room() {
    run_test(
        "personal_transfer_move_waits_for_a_closing_source_room",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
            let (_, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
            let hub = run.hub();
            let key = fvoci_server::collab::room::RoomKey::task(source, task);
            // Idle eviction of the live source room is held just before its actor
            // releases the resource guard (the slot is Closing).
            let (reached, proceed) = fvoci_server::collab::room::arm_teardown_barrier(task).await;
            let evictor = {
                let hub = hub.clone();
                tokio::spawn(async move {
                    hub.force_room_idle_eligible(key).await;
                    hub.execute_idle_evict_if_eligible(key).await
                })
            };
            reached.await.unwrap();
            let body = move_body(addr, &actor, source, &selection).await;
            let mover = spawn_move(addr, &actor, source, body);
            assert_move_waits(&hub, key, &mover, "a closing source room").await;
            let _ = proceed.send(());
            assert!(evictor.await.unwrap());
            let (status, moved) = mover.await.unwrap();
            assert_eq!(status, StatusCode::OK, "{moved}");
            peer_edits_moved_task(addr, &run, actor.workspace_id, project, task).await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_concurrent_move_replay_retires_once_and_peer_joins() {
    run_test(
        "personal_transfer_concurrent_move_replay_retires_once_and_peer_joins",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
            let (_, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
            let body = move_body(addr, &actor, source, &selection).await;
            let (first, second) = tokio::join!(
                spawn_move(addr, &actor, source, body.clone()),
                spawn_move(addr, &actor, source, body)
            );
            let (first, second) = (first.unwrap(), second.unwrap());
            assert_eq!(
                (first.0, second.0),
                (StatusCode::OK, StatusCode::OK),
                "{} {}",
                first.1,
                second.1
            );
            assert_eq!(first.1["taskId"], second.1["taskId"]);
            let replays = [
                first.1["replayed"].as_bool(),
                second.1["replayed"].as_bool(),
            ];
            assert!(
                replays.contains(&Some(true)) && replays.contains(&Some(false)),
                "{replays:?}"
            );
            peer_edits_moved_task(addr, &run, actor.workspace_id, project, task).await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_source_join_admitted_before_move_cannot_hold_the_room() {
    run_test(
        "personal_transfer_source_join_admitted_before_move_cannot_hold_the_room",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
            let (_, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
            let hub = run.hub();
            let key = fvoci_server::collab::room::RoomKey::task(source, task);
            hub.force_room_idle_eligible(key).await;
            assert!(
                hub.execute_idle_evict_if_eligible(key).await,
                "fixture: the idle source room is evicted"
            );
            // Admitted in the source before the MOVE commits; it starts its room
            // only after the MOVE answered.
            let (reached, proceed) = fvoci_server::collab::hub::arm_hub_join_barrier(
                task,
                fvoci_server::collab::hub::HUB_JOIN_BARRIER_AFTER_ADMISSION,
            )
            .await;
            let source_join = tokio::spawn(join_reply(
                addr,
                actor.session_token.clone(),
                task_key(source, task),
                212,
            ));
            reached.await.unwrap();
            let body = move_body(addr, &actor, source, &selection).await;
            let (status, moved) = spawn_move(addr, &actor, source, body).await.unwrap();
            assert_eq!(status, StatusCode::OK, "{moved}");
            let _ = proceed.send(());
            assert!(
                source_join.await.unwrap().is_err(),
                "the old scope join is refused"
            );
            // The late source start must not keep the resource guard.
            peer_edits_moved_task(addr, &run, actor.workspace_id, project, task).await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

/// RQ3: retained tail (applied, not persisted) and compacted receipts on both
/// targets move byte-identical; both targets read independently; a distinct
/// member restores a moved document and task revision; a guest and the old
/// scope are refused.
#[tokio::test]
async fn personal_transfer_move_retained_tail_restores_and_scope_denials() {
    run_test("personal_transfer_move_retained_tail_restores_and_scope_denials", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
        let (document, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
        let team = actor.workspace_id;
        let doc_tail = seed_update(&run, json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"문서 꼬리 편집")]})).await;
        let doc_ws = apply_without_persist(addr, &actor.session_token, &document_key(source, document), 241, &doc_tail).await;
        let task_tail = seed_update(&run, json!({"type":"doc","content":[para(&Uuid::now_v7().to_string(),"작업 꼬리 편집")]})).await;
        let task_ws = apply_without_persist(addr, &actor.session_token, &task_key(source, task), 242, &task_tail).await;
        selection_versions(&run, source, &mut selection).await;
        let before = transfer_graph(&run, source).await;
        for (updates, receipts, column, id) in [
            ("document_collab_updates", "document_collab_op_receipts", "document_id", document),
            ("task_collab_updates", "task_collab_op_receipts", "task_id", task),
        ] {
            let tail = rows_of(&before, updates, column, id, None).len();
            let all = rows_of(&before, receipts, column, id, None).len();
            assert!(tail >= 1, "{updates}: a retained tail");
            assert!(all > tail, "{receipts}: compacted receipts besides the tail");
        }
        let body = move_body(addr, &actor, source, &selection).await;
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        drop((doc_ws, task_ws));
        let after_team = transfer_graph(&run, team).await;
        let after_source = transfer_graph(&run, source).await;
        let mut tables: Vec<(&str, &str, Uuid)> = NATIVE_TABLES
            .iter()
            .map(|(table, column, _)| (*table, *column, if *column == "document_id" { document } else { task }))
            .collect();
        tables.push(("revisions", "target_id", document));
        tables.push(("revisions", "target_id", task));
        for (table, column, id) in tables {
            assert_eq!(rows_of(&after_team, table, column, id, None), rows_of(&before, table, column, id, Some(team)), "{table} {id}");
            assert!(rows_of(&after_source, table, column, id, None).is_empty(), "{table} left in source");
        }
        // Independent reads of both moved targets: state + retained tail.
        let hub = run.hub();
        let mut conn = scoped_observer(&run, team).await;
        for (kind, states, updates, column, id) in [
            ("documents", "document_states", "document_collab_updates", "document_id", document),
            ("tasks", "task_states", "task_collab_updates", "task_id", task),
        ] {
            let (state, stored): (Vec<u8>, Value) = sqlx::query_as(&format!("SELECT s.state, r.content_json FROM fvoci.{states} s JOIN fvoci.{kind} r ON r.workspace_id=s.workspace_id AND r.id=s.{column} WHERE s.workspace_id=$1 AND s.{column}=$2"))
                .bind(team).bind(id).fetch_one(&mut conn).await.unwrap();
            let tail: Vec<Vec<u8>> = sqlx::query_scalar(&format!("SELECT payload FROM fvoci.{updates} WHERE workspace_id=$1 AND {column}=$2 ORDER BY seq"))
                .bind(team).bind(id).fetch_all(&mut conn).await.unwrap();
            assert!(!tail.is_empty());
            let projected = fvoci_server::collab::revision::project_persisted_offline(hub.engine_bin(), hub.limits(), state, tail).unwrap();
            assert_eq!(projected, stored, "{kind}");
        }
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();
        // A distinct member restores a moved document and task revision.
        let peer = create_user_session(&run.harness, team, WorkspaceRole::Member).await;
        add_project_member(&run.harness, team, project, peer.user_id, "member").await;
        let admin = admin_pool(&run.harness).await;
        let document_revision = rows_of(&after_team, "revisions", "target_id", document, None)[0]["id"].as_str().unwrap().to_string();
        let task_revision = rows_of(&after_team, "revisions", "target_id", task, None)[0]["id"].as_str().unwrap().to_string();
        for restore in [
            format!("/api/v1/workspaces/{team}/projects/{project}/documents/{document}/revisions/{document_revision}/restore"),
            task_path(&peer, task, &format!("/revisions/{task_revision}/restore")),
        ] {
            let (status, restored) = previewed_restore(addr, &restore, &peer.session_token).await;
            assert_eq!(status, StatusCode::OK, "{restore}: {restored}");
            assert_eq!(restored["restored"], true);
        }
        let (content, _, _) = wait_task_content_contains(&admin, team, task, "이동할 작업 본문").await;
        assert!(!content.to_string().contains("작업 꼬리 편집"), "{content}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let body: Value = sqlx::query_scalar("SELECT content_json FROM fvoci.documents WHERE workspace_id=$1 AND id=$2")
                .bind(team).bind(document).fetch_one(&admin).await.unwrap();
            let text = body.to_string();
            if text.contains("이동할 문서 본문") && !text.contains("문서 꼬리 편집") {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "document restore: {text}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        admin.close().await;
        // A guest of the team and the old private scope are refused.
        let guest = create_user_session(&run.harness, team, WorkspaceRole::Guest).await;
        let (status, _) = session_call(addr, Method::GET, &task_path(&guest, task, "/revisions?limit=20"), &guest.session_token, None).await;
        assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "guest revisions: {status}");
        assert!(join_reply(addr, guest.session_token.clone(), task_key(team, task), 243).await.is_err(), "guest join refused");
        let (status, _) = session_call(addr, Method::GET, &document_api(source, document, "/revisions?limit=20"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "old scope revisions");
        assert!(join_reply(addr, actor.session_token.clone(), document_key(source, document), 244).await.is_err(), "old scope join refused");
        run.finish().await.unwrap();
    })
    .await;
}

/// Pair retirement under caller cancellation (reviewer ctx839): a MOVE is
/// committed (database layer, as a request cancelled right after its commit
/// would leave it), the document's source room is held Closing, and the
/// caller of the pair retirement is aborted while it waits on the document.
/// Within a bounded budget, and with idle eviction of the task held off (no
/// masking), the moved task's source room must still be retired and a
/// distinct team member must be able to join it. Caller-future cancellation
/// only; a real network disconnect reaching the handler is not shown here.
#[tokio::test]
async fn personal_transfer_cancelled_retirement_caller_still_retires_both_rooms() {
    run_test(
        "personal_transfer_cancelled_retirement_caller_still_retires_both_rooms",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
            let (document, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
            let team = actor.workspace_id;
            let hub = run.hub();
            let document_key = fvoci_server::collab::room::RoomKey::document(source, document);
            let task_key_source = fvoci_server::collab::room::RoomKey::task(source, task);
            let _held = HeldIdleEviction::new(task);
            let body = move_body(addr, &actor, source, &selection).await;
            let parsed: fvoci_server::api::personal_transfer::PersonalTransferBody =
                serde_json::from_value(body).unwrap();
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.app_url)
                .await
                .unwrap();
            let engine = fvoci_server::db::personal_transfer::TransferBodyEngine {
                engine_bin: hub.engine_bin(),
                limits: hub.limits(),
            };
            let moved = fvoci_server::db::personal_transfer::transfer_personal_item(
                &pool,
                source,
                actor.user_id,
                actor.session_id,
                &parsed,
                None,
                "web",
                Some(&engine),
                None,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(moved.task_id, Some(task));
            pool.close().await;
            // The document's source room is held Closing before its guard release.
            let (reached, proceed) = fvoci_server::collab::room::arm_teardown_barrier(document).await;
            let evictor = {
                let hub = hub.clone();
                tokio::spawn(async move {
                    hub.force_room_idle_eligible(document_key).await;
                    hub.execute_idle_evict_if_eligible(document_key).await
                })
            };
            reached.await.unwrap();
            assert!(hub.room_occupies_slot(task_key_source).await, "fixture: the task source room is live");
            let caller = {
                let hub = hub.clone();
                tokio::spawn(async move {
                    hub.retire_moved_resource_rooms(vec![document_key, task_key_source]).await
                })
            };
            while hub.room_waiter_count(document_key).await == 0 {
                tokio::task::yield_now().await;
            }
            caller.abort();
            let _ = caller.await;
            let _ = proceed.send(());
            assert!(evictor.await.unwrap());
            let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
            while hub.room_occupies_slot(task_key_source).await {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the moved task's source room still holds the guard after the cancelled retirement caller"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            peer_edits_moved_task(addr, &run, team, project, task).await;
            run.finish().await.unwrap();
        },
    )
    .await;
}

/// Test-scoped idle-eviction hold, released on success and on panic.
struct HeldIdleEviction(Uuid);

impl HeldIdleEviction {
    fn new(resource: Uuid) -> Self {
        fvoci_server::collab::hub::hold_idle_eviction(resource);
        Self(resource)
    }
}

impl Drop for HeldIdleEviction {
    fn drop(&mut self) {
        fvoci_server::collab::hub::release_idle_eviction(self.0);
    }
}

fn read_var_uint(bytes: &[u8]) -> (u64, &[u8]) {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return (value, &bytes[index + 1..]);
        }
    }
    panic!("truncated varUint");
}

/// A new native client: joins the room, asks for everything from an empty
/// state vector, and projects the server's Step2 update with the engine.
async fn native_client_read(
    addr: SocketAddr,
    run: &TestRun,
    token: &str,
    key: &str,
    client: u32,
) -> Value {
    let mut ws = connect_member(addr, token).await;
    auth_and_join(&mut ws, key, client).await;
    ws.send(Message::Binary(
        support::sync_step1_frame(key, &[0, 0]).into(),
    ))
    .await
    .unwrap();
    let mut update = None;
    for _ in 0..16 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(fvoci_server::collab::wire::SyncMessage {
                    step: fvoci_server::collab::wire::SyncStep::Step2,
                    y_protocol,
                }),
            ..
        }) = support::recv_document_frame(&mut ws, 1).await
        {
            update = Some(y_protocol);
            break;
        }
    }
    let y_protocol = update.expect("server Step2");
    let (_, rest) = read_var_uint(&y_protocol);
    let (len, rest) = read_var_uint(rest);
    let state = rest[..len as usize].to_vec();
    let hub = run.hub();
    fvoci_server::collab::revision::project_persisted_offline(
        hub.engine_bin(),
        hub.limits(),
        state,
        vec![],
    )
    .unwrap()
}

fn text(value: &str) -> Value {
    json!({"type":"text","text":value})
}

/// Bold text as written by clients (editor input form).
fn bold(value: &str) -> Value {
    json!({"type":"text","text":value,"marks":[{"type":"bold"}]})
}

/// Bold text as the engine's native projection writes it: marks always carry
/// an attrs object (crates/collab-engine/fixtures/expectations.json and the
/// src/project.rs tests), unlike the editor input form above.
fn bold_native(value: &str) -> Value {
    json!({"type":"text","text":value,"marks":[{"type":"bold","attrs":{}}]})
}

fn block(id: Uuid, content: Vec<Value>) -> Value {
    json!({"type":"paragraph","attrs":{"id":id.to_string()},"content":content})
}

/// Joins `key` and keeps the socket open (so the room never closes on a
/// last disconnect, which would compact the retained tail).
async fn hold_room(addr: SocketAddr, token: &str, key: &str, client: u32) -> Ws {
    let mut ws = connect_member(addr, token).await;
    auth_and_join(&mut ws, key, client).await;
    complete_sync_handshake(&mut ws, key).await;
    ws
}

/// RQ2/RQ3 (a945 and 9b89 reviews): every expected state is an independently
/// authored literal (structure, attrs, marks), never read back from the
/// system. Document: body replaced by A (product PUT), revision, persist
/// (A's op becomes a compacted receipt), body replaced by B (retained tail).
/// Task: native state seeded with A (fixture: the capture task has no block
/// ids to patch), revision, block patch, persist, block patch -> B (retained
/// tail). After MOVE, new native clients read exactly B; after a distinct
/// member restores the moved revisions, exactly A. Every restore with a known
/// revision id from the old private scope or by a team guest is refused and
/// leaves both workspaces' complete transfer graphs unchanged.
#[tokio::test]
async fn personal_transfer_move_restores_exact_history_for_new_clients() {
    run_test("personal_transfer_move_restores_exact_history_for_new_clients", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, project, _, mut selection) = setup_transfer(&mut run).await;
        let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
        let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
        let team = actor.workspace_id;
        let (d1, d2, t1, t2) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let doc_a = json!({"type":"doc","content":[block(d1, vec![text("원본 문서 첫 단락 "), bold("굵게 🧑‍💻")]), block(d2, vec![text("둘째 단락 日本語 text")])]});
        let doc_b = json!({"type":"doc","content":[block(d1, vec![text("이동 직전 문서 단락")]), block(d2, vec![text("꼬리 단락 한글 "), bold("끝")])]});
        let task_a = json!({"type":"doc","content":[block(t1, vec![text("원본 작업 단락 한글")]), block(t2, vec![text("작업 둘째 "), bold("日本")])]});
        // The same authored states as a new native client must read them.
        let doc_a_native = json!({"type":"doc","content":[block(d1, vec![text("원본 문서 첫 단락 "), bold_native("굵게 🧑‍💻")]), block(d2, vec![text("둘째 단락 日本語 text")])]});
        let doc_b_native = json!({"type":"doc","content":[block(d1, vec![text("이동 직전 문서 단락")]), block(d2, vec![text("꼬리 단락 한글 "), bold_native("끝")])]});
        let task_a_native = json!({"type":"doc","content":[block(t1, vec![text("원본 작업 단락 한글")]), block(t2, vec![text("작업 둘째 "), bold_native("日本")])]});
        let task_b1_native = json!({"type":"doc","content":[block(t1, vec![text("중간 작업 단락")]), block(t2, vec![text("작업 둘째 "), bold_native("日本")])]});
        let task_b_native = json!({"type":"doc","content":[block(t1, vec![text("중간 작업 단락")]), block(t2, vec![text("꼬리 작업 단락 "), bold_native("끝")])]});
        let personal = in_personal(&actor, source);
        // Document A (product PUT), revision, compaction, B in the tail.
        let (status, put) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson": doc_a}))).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        let (status, created) = session_call(addr, Method::POST, &document_api(source, document, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let mut doc_room = hold_room(addr, &actor.session_token, &document_key(source, document), 271).await;
        persist_once(&mut doc_room, &document_key(source, document)).await;
        let (status, put) = session_call(addr, Method::PUT, &document_api(source, document, "/body"), &actor.session_token, Some(json!({"contentJson": doc_b}))).await;
        assert_eq!(status, StatusCode::OK, "{put}");
        // Task A (fixture native state + stored body), revision, patch,
        // compaction, patch -> B in the tail.
        let admin = admin_pool(&run.harness).await;
        let seed = seed_update(&run, task_a.clone()).await;
        sqlx::query("INSERT INTO fvoci.task_states (workspace_id, task_id, state, encoding) VALUES ($1,$2,$3,1)")
            .bind(source).bind(task).bind(&seed).execute(&admin).await.unwrap();
        sqlx::query("UPDATE fvoci.tasks SET content_json=$3 WHERE workspace_id=$1 AND id=$2")
            .bind(source).bind(task).bind(&task_a).execute(&admin).await.unwrap();
        let (status, created) = session_call(addr, Method::POST, &task_path(&personal, task, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let mut task_room = hold_room(addr, &actor.session_token, &task_key(source, task), 272).await;
        let patch = |id: Uuid, content: Vec<Value>| json!({"type":"paragraph","attrs":{"id":id.to_string()},"content":content});
        let (status, patched) = session_call(addr, Method::PATCH, &task_path(&personal, task, &format!("/blocks/{t1}")), &actor.session_token, Some(patch(t1, vec![text("중간 작업 단락")]))).await;
        assert_eq!(status, StatusCode::OK, "{patched}");
        assert_eq!(native_client_read(addr, &run, &actor.session_token, &task_key(source, task), 273).await, task_b1_native);
        persist_once(&mut task_room, &task_key(source, task)).await;
        let (status, patched) = session_call(addr, Method::PATCH, &task_path(&personal, task, &format!("/blocks/{t2}")), &actor.session_token, Some(patch(t2, vec![text("꼬리 작업 단락 "), bold("끝")]))).await;
        assert_eq!(status, StatusCode::OK, "{patched}");
        assert_eq!(native_client_read(addr, &run, &actor.session_token, &document_key(source, document), 274).await, doc_b_native, "source document before MOVE");
        assert_eq!(native_client_read(addr, &run, &actor.session_token, &task_key(source, task), 275).await, task_b_native, "source task before MOVE");
        selection_versions(&run, source, &mut selection).await;
        let before = transfer_graph(&run, source).await;
        for (updates, receipts, column, id) in [
            ("document_collab_updates", "document_collab_op_receipts", "document_id", document),
            ("task_collab_updates", "task_collab_op_receipts", "task_id", task),
        ] {
            let tail = rows_of(&before, updates, column, id, None).len();
            assert!(tail >= 1, "{updates}: a retained tail");
            assert!(rows_of(&before, receipts, column, id, None).len() > tail, "{receipts}: compacted receipts besides the tail");
        }
        let body = move_body(addr, &actor, source, &selection).await;
        let (status, moved) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{moved}");
        drop((doc_room, task_room));
        let peer = create_user_session(&run.harness, team, WorkspaceRole::Member).await;
        add_project_member(&run.harness, team, project, peer.user_id, "member").await;
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &document_key(team, document), 276).await, doc_b_native, "moved document, new client");
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &task_key(team, task), 277).await, task_b_native, "moved task, new client");
        let graph = transfer_graph(&run, team).await;
        let document_revision = rows_of(&graph, "revisions", "target_id", document, None)[0]["id"].as_str().unwrap().to_string();
        let task_revision = rows_of(&graph, "revisions", "target_id", task, None)[0]["id"].as_str().unwrap().to_string();
        // Known revision ids from the old private scope and by a team guest.
        let guest = create_user_session(&run.harness, team, WorkspaceRole::Guest).await;
        for (route, token) in [
            (document_api(source, document, &format!("/revisions/{document_revision}/restore")), actor.session_token.clone()),
            (task_path(&personal, task, &format!("/revisions/{task_revision}/restore")), actor.session_token.clone()),
            (format!("/api/v1/workspaces/{team}/projects/{project}/documents/{document}/revisions/{document_revision}/restore"), guest.session_token.clone()),
            (task_path(&guest, task, &format!("/revisions/{task_revision}/restore")), guest.session_token.clone()),
        ] {
            let (team_before, source_before) = (transfer_graph(&run, team).await, transfer_graph(&run, source).await);
            // A well-formed body (the W4 restore contract requires expectedTailSeq), so
            // the refusal is the scope check, not body parsing.
            let (status, refused) = session_call(addr, Method::POST, &route, &token, Some(json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": "0"}))).await;
            assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND), "{route}: {status} {refused}");
            assert_eq!(transfer_graph(&run, team).await, team_before, "{route}: team graph unchanged");
            assert_eq!(transfer_graph(&run, source).await, source_before, "{route}: source graph unchanged");
        }
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &document_key(team, document), 278).await, doc_b_native);
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &task_key(team, task), 279).await, task_b_native);
        for route in [
            format!("/api/v1/workspaces/{team}/projects/{project}/documents/{document}/revisions/{document_revision}/restore"),
            task_path(&peer, task, &format!("/revisions/{task_revision}/restore")),
        ] {
            let (status, restored) = previewed_restore(addr, &route, &peer.session_token).await;
            assert_eq!(status, StatusCode::OK, "{route}: {restored}");
            assert_eq!(restored["restored"], true);
        }
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &document_key(team, document), 280).await, doc_a_native, "restored document, new client");
        assert_eq!(native_client_read(addr, &run, &peer.session_token, &task_key(team, task), 281).await, task_a_native, "restored task, new client");
        admin.close().await;
        run.finish().await.unwrap();
    })
    .await;
}

/// Genuine native rows for one target, written the way a room writes them
/// but without a room: one engine child per compaction cycle loads the
/// committed snapshot and tail. Setup content: ONE genuine authored update
/// per target (SeedFromTiptap of a fresh one-paragraph doc, applied once;
/// or `replay`, the bytes a previous call authored) re-appended under fresh
/// op_ids - a re-delivered idempotent update, not distinct edits, so the
/// native state stays bounded (b2fea00/02c9a29/1056b73 measured distinct
/// paragraphs growing the engine cost and reaching the W7 per-target bound).
/// Before the product tail cap (64) the local complete snapshot is compacted
/// through the product compact function at the actual cutoff. It always ends
/// compacted, then the product claim reloads the target, its engine
/// projection must hold exactly the paragraphs present before the call plus
/// the authored one (block id and content, each once; CRDT order not
/// asserted) within the product body bound, and the product derived-body
/// write stores that projection (what a room does after its updates).
/// Returns the update bytes used.
///
/// `batch = false` appends each update through the product append function
/// (the landing path). `batch = true` is setup only, not writer-throughput or
/// WebSocket evidence: each cycle's updates (at most 63, tail + batch < 64)
/// are written in ONE restricted-role transaction with the product
/// statements (fenced state row, tail_seq += n, updates, receipts with the
/// exact length and SHA-256 of each payload, and the unchanged per-append
/// event/audit through `record_collab_append_for_tests`), then read back and
/// checked before the product compaction.
#[allow(clippy::too_many_arguments)]
async fn drive_native_rows(
    run: &TestRun,
    actor: &SessionFixture,
    source: Uuid,
    kind: CollabKind,
    target: Uuid,
    appends: usize,
    label: &'static str,
    batch: bool,
    replay: Option<Vec<u8>>,
) -> Vec<u8> {
    use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
    use collab_engine::protocol::Request;
    use collab_engine::EngineStatus;
    use fvoci_server::db::collab::{
        append_collab_update_kind, claim_writer_and_load_kind, compact_collab_snapshot_kind,
        record_collab_append_for_tests, AppendCollabInput, AppendCollabResult, CompactCollabInput,
        MAX_COLLAB_LOAD_BYTES, MAX_COLLAB_TAIL_UPDATES, MAX_COLLAB_UPDATE_BYTES,
    };
    use sha2::{Digest, Sha256};
    fn bytes(report: collab_engine::EngineReport, step: &str) -> Vec<u8> {
        match report.outcome {
            EngineStatus::Ok {
                update_b64: Some(b64),
                ..
            } => collab_engine::b64::decode(&b64).expect("engine bytes"),
            other => panic!("engine {step}: {other:?}"),
        }
    }
    fn projection(engine: &mut EngineSession) -> Value {
        match engine.call(&Request::Project { encoding: 1 }).outcome {
            EngineStatus::Ok {
                content_json: Some(json),
                ..
            } => json,
            other => panic!("engine project: {other:?}"),
        }
    }
    /// A paragraph as the order-free oracle compares it: block id + content.
    fn identity(node: &Value) -> String {
        json!({"id": node["attrs"]["id"], "content": node["content"]}).to_string()
    }
    fn ok(report: collab_engine::EngineReport, step: &str) {
        assert!(
            matches!(report.outcome, EngineStatus::Ok { .. }),
            "engine {step}: {:?}",
            report.outcome
        );
    }
    let (states, updates, receipts, id_column) = match kind {
        CollabKind::Document => (
            "document_states",
            "document_collab_updates",
            "document_collab_op_receipts",
            "document_id",
        ),
        _ => (
            "task_states",
            "task_collab_updates",
            "task_collab_op_receipts",
            "task_id",
        ),
    };
    let hub = run.hub();
    let (engine_bin, limits) = (hub.engine_bin(), hub.limits());
    let (pool, user, session) = (actor.pool.clone(), actor.user_id, actor.session_id);
    let handle = tokio::runtime::Handle::current();
    let spawn = move || {
        EngineSession::spawn(SpawnRequest {
            engine_bin: engine_bin.clone(),
            limits,
            slot_kind: ChildSlotKind::Seed,
            slot_wait: Some(Duration::from_secs(10)),
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .expect("engine session")
    };
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let claim = handle
            .block_on(claim_writer_and_load_kind(&pool, kind, source, user, session, target))
            .unwrap()
            .expect("writer claim");
        let generation = claim.writer_generation;
        let mut snapshot = claim.load.snapshot;
        let mut tail: Vec<Vec<u8>> = claim.load.tail.into_iter().map(|row| row.payload).collect();
        let mut tail_seq = claim.load.tail_seq;
        let (mut done, mut cycles) = (0usize, 0usize);
        // Order-free content oracle: the paragraphs present before this call
        // plus every authored (fresh block, text), each exactly once.
        // Independent updates from distinct clients merge in CRDT order,
        // not arrival order, so position is not asserted.
        let mut initial: Option<Vec<String>> = None;
        let mut authored: Vec<String> = Vec::new();
        let mut update = replay;
        while done < appends || !tail.is_empty() {
            // Diagnostic timing only (no behaviour change): per cycle.
            let cycle_started = std::time::Instant::now();
            let loaded_bytes = snapshot.len();
            let mut engine = spawn();
            ok(
                engine.call(&Request::Load {
                    snapshot_b64: Some(snapshot.clone()),
                    tail_b64: tail.clone(),
                    encoding: 1,
                }),
                "load",
            );
            if initial.is_none() {
                let nodes = projection(&mut engine)["content"].as_array().cloned().unwrap_or_default();
                initial = Some(nodes.iter().map(identity).collect());
            }
            let load_ms = cycle_started.elapsed().as_secs_f64() * 1000.0;
            let steps_started = std::time::Instant::now();
            let mut pending: Vec<(Uuid, Vec<u8>)> = Vec::new();
            while done < appends && tail.len() + pending.len() < 63 {
                // ONE genuine authored update per target (a fresh one-paragraph
                // SeedFromTiptap update, applied once); every further operation
                // re-appends those identical bytes under a fresh op_id - a
                // re-delivered idempotent update, not a distinct edit.
                let forward = match &update {
                    Some(bytes) => bytes.clone(),
                    None => {
                        let text = format!("{label} 행");
                        let block = Uuid::now_v7().to_string();
                        let content = json!({"type":"doc","content":[para(&block, &text)]});
                        let forward = bytes(
                            engine.call(&Request::SeedFromTiptap {
                                content_json: content.to_string(),
                                encoding: 1,
                            }),
                            "seed",
                        );
                        let applied = engine.call(&Request::Apply {
                            update_b64: forward.clone(),
                            encoding: 1,
                        });
                        assert!(applied.outcome.is_applied_ok(), "engine apply: {:?}", applied.outcome);
                        authored.push(identity(&para(&block, &text)));
                        update = Some(forward.clone());
                        forward
                    }
                };
                done += 1;
                if !batch {
                    let appended = handle
                        .block_on(append_collab_update_kind(
                            &pool,
                            kind,
                            AppendCollabInput {
                                workspace_id: source,
                                actor_user_id: user,
                                session_id: session,
                                document_id: target,
                                writer_generation: generation,
                                expected_tail_seq: tail_seq,
                                op_id: Uuid::now_v7(),
                                payload: &forward,
                                client_ip: None,
                            },
                        ))
                        .unwrap()
                        .expect("product append");
                    let AppendCollabResult::Committed { seq } = appended else {
                        panic!("append not committed: {appended:?}");
                    };
                    tail_seq = seq;
                    tail.push(forward);
                } else {
                    pending.push((Uuid::now_v7(), forward));
                }
            }
            let steps_ms = steps_started.elapsed().as_secs_f64() * 1000.0;
            let batch_count = pending.len();
            let (mut write_ms, mut readback_ms) = (0.0, 0.0);
            if !pending.is_empty() {
                // One transaction for the cycle's genuine updates.
                let count = pending.len() as i64;
                assert!((tail.len() as i64) + count < MAX_COLLAB_TAIL_UPDATES, "batch within the tail cap");
                let batch_bytes: i64 = pending.iter().map(|(_, p)| p.len() as i64).sum();
                assert!(pending.iter().all(|(_, p)| !p.is_empty() && p.len() <= MAX_COLLAB_UPDATE_BYTES));
                let seqs: Vec<i64> = (1..=count).map(|i| tail_seq + i).collect();
                let ops: Vec<Uuid> = pending.iter().map(|(op, _)| *op).collect();
                let payloads: Vec<Vec<u8>> = pending.iter().map(|(_, p)| p.clone()).collect();
                let lens: Vec<i64> = payloads.iter().map(|p| p.len() as i64).collect();
                let hashes: Vec<Vec<u8>> = payloads.iter().map(|p| Sha256::digest(p).to_vec()).collect();
                (write_ms, readback_ms) = handle.block_on(async {
                    let write_started = std::time::Instant::now();
                    let mut tx = pool.begin().await.unwrap();
                    fvoci_server::db::context::set_tenant(&mut tx, source).await.unwrap();
                    let (current_generation, current_tail, cutoff, snapshot_len): (i64, i64, i64, i64) = sqlx::query_as(&format!(
                        "SELECT writer_generation, tail_seq, snapshot_cutoff_seq, octet_length(state)::bigint FROM fvoci.{states} WHERE workspace_id=$1 AND {id_column}=$2 FOR UPDATE"
                    ))
                    .bind(source)
                    .bind(target)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    assert_eq!((current_generation, current_tail), (generation, tail_seq), "writer fence");
                    let (tail_rows, tail_bytes): (i64, i64) = sqlx::query_as(&format!(
                        "SELECT count(*)::bigint, coalesce(sum(octet_length(payload)),0)::bigint FROM fvoci.{updates} WHERE workspace_id=$1 AND {id_column}=$2 AND seq > $3"
                    ))
                    .bind(source)
                    .bind(target)
                    .bind(cutoff)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    assert!(tail_rows + count < MAX_COLLAB_TAIL_UPDATES, "tail + batch < cap");
                    assert!(snapshot_len + tail_bytes + batch_bytes <= MAX_COLLAB_LOAD_BYTES, "load bytes bound");
                    let moved: i64 = sqlx::query_scalar(&format!(
                        "UPDATE fvoci.{states} SET tail_seq = tail_seq + $4, updated_at = now() WHERE workspace_id=$1 AND {id_column}=$2 AND writer_generation=$3 RETURNING tail_seq"
                    ))
                    .bind(source)
                    .bind(target)
                    .bind(generation)
                    .bind(count)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    assert_eq!(moved, tail_seq + count);
                    sqlx::query(&format!(
                        "INSERT INTO fvoci.{updates}(workspace_id,{id_column},seq,op_id,payload) SELECT $1,$2,s,o,p FROM unnest($3::bigint[],$4::uuid[],$5::bytea[]) AS b(s,o,p)"
                    ))
                    .bind(source)
                    .bind(target)
                    .bind(&seqs)
                    .bind(&ops)
                    .bind(&payloads)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                    sqlx::query(&format!(
                        "INSERT INTO fvoci.{receipts}(workspace_id,{id_column},op_id,seq,payload_len,payload_sha256,actor_user_id) SELECT $1,$2,o,s,l,h,$7 FROM unnest($3::uuid[],$4::bigint[],$5::bigint[],$6::bytea[]) AS b(o,s,l,h)"
                    ))
                    .bind(source)
                    .bind(target)
                    .bind(&ops)
                    .bind(&seqs)
                    .bind(&lens)
                    .bind(&hashes)
                    .bind(user)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                    for (op, seq) in ops.iter().zip(&seqs) {
                        record_collab_append_for_tests(&mut tx, kind, source, user, target, *op, *seq, generation)
                            .await
                            .unwrap();
                    }
                    tx.commit().await.unwrap();
                    let write_ms = write_started.elapsed().as_secs_f64() * 1000.0;
                    let readback_started = std::time::Instant::now();
                    // Read back: contiguous seqs, exact payloads, receipts
                    // equal to each stored payload's length and SHA-256.
                    let mut check = pool.begin().await.unwrap();
                    fvoci_server::db::context::set_tenant(&mut check, source).await.unwrap();
                    type Stored = (i64, Uuid, Vec<u8>, i64, Vec<u8>, Uuid);
                    let stored: Vec<Stored> = sqlx::query_as(&format!(
                        "SELECT u.seq, u.op_id, u.payload, r.payload_len, r.payload_sha256, r.actor_user_id FROM fvoci.{updates} u JOIN fvoci.{receipts} r ON r.workspace_id=u.workspace_id AND r.{id_column}=u.{id_column} AND r.seq=u.seq AND r.op_id=u.op_id WHERE u.workspace_id=$1 AND u.{id_column}=$2 AND u.seq > $3 AND u.seq <= $4 ORDER BY u.seq"
                    ))
                    .bind(source)
                    .bind(target)
                    .bind(tail_seq)
                    .bind(tail_seq + count)
                    .fetch_all(&mut *check)
                    .await
                    .unwrap();
                    check.rollback().await.unwrap();
                    assert_eq!(stored.len() as i64, count, "every update has its receipt");
                    for (i, (seq, op, payload, len, hash, actor_id)) in stored.into_iter().enumerate() {
                        assert_eq!((seq, op, actor_id), (seqs[i], ops[i], user));
                        assert_eq!(payload, payloads[i]);
                        assert_eq!((len, hash), (payload.len() as i64, Sha256::digest(&payload).to_vec()));
                    }
                    (write_ms, readback_started.elapsed().as_secs_f64() * 1000.0)
                });
                tail_seq += count;
                tail.extend(payloads);
            }
            let snapshot_started = std::time::Instant::now();
            snapshot = bytes(engine.call(&Request::Snapshot), "snapshot");
            let snapshot_ms = snapshot_started.elapsed().as_secs_f64() * 1000.0;
            let compact_started = std::time::Instant::now();
            handle
                .block_on(compact_collab_snapshot_kind(
                    &pool,
                    kind,
                    CompactCollabInput {
                        workspace_id: source,
                        actor_user_id: user,
                        session_id: session,
                        document_id: target,
                        writer_generation: generation,
                        cutoff_seq: tail_seq,
                        expected_tail_seq: tail_seq,
                        new_snapshot: &snapshot,
                        client_ip: None,
                    },
                ))
                .unwrap()
                .expect("product compaction");
            let compact_ms = compact_started.elapsed().as_secs_f64() * 1000.0;
            if batch {
                eprintln!(
                    "[budget-cycle] {label} c={} n={batch_count} loaded_bytes={loaded_bytes} load_ms={load_ms:.1} steps_ms={steps_ms:.1} write_ms={write_ms:.1} readback_ms={readback_ms:.1} snapshot_ms={snapshot_ms:.1} snapshot_bytes={} compact_ms={compact_ms:.1} cycle_ms={:.1}",
                    cycles + 1,
                    snapshot.len(),
                    cycle_started.elapsed().as_secs_f64() * 1000.0
                );
            }
            tail.clear();
            cycles += 1;
            if cycles.is_multiple_of(16) || done == appends {
                eprintln!(
                    "[budget-diag] {label}: {done} of {appends} appended, {cycles} compactions after {:.3}s",
                    started.elapsed().as_secs_f64()
                );
            }
        }
        if appends > 0 {
            // The product claim loads what was written; the engine projects
            // it (its last paragraph is the last authored one), and the
            // product writes that projection as the derived body, as a room
            // does after its updates.
            let reloaded = handle
                .block_on(claim_writer_and_load_kind(&pool, kind, source, user, session, target))
                .unwrap()
                .expect("product reload");
            let mut engine = spawn();
            ok(
                engine.call(&Request::Load {
                    snapshot_b64: Some(reloaded.load.snapshot),
                    tail_b64: reloaded.load.tail.into_iter().map(|row| row.payload).collect(),
                    encoding: 1,
                }),
                "reload",
            );
            let projected = projection(&mut engine);
            let mut present: Vec<String> = projected["content"].as_array().map(|nodes| nodes.iter().map(identity).collect()).unwrap_or_default();
            let mut expected: Vec<String> = initial.clone().unwrap_or_default();
            expected.extend(authored.iter().cloned());
            present.sort();
            expected.sort();
            let missing = expected.iter().filter(|e| present.binary_search(e).is_err()).count();
            let extra = present.iter().filter(|p| expected.binary_search(p).is_err()).count();
            assert_eq!(
                (present.len(), missing, extra),
                (expected.len(), 0, 0),
                "every authored paragraph exactly once and the initial paragraphs unchanged"
            );
            assert!(present == expected, "paragraph multiset equal (no duplicates)");
            assert!(
                projected.to_string().len() < fvoci_server::collab::derived_body::DOCUMENT_MAX_BODY_BYTES,
                "derived body within the product bound"
            );
            let prepared = fvoci_server::collab::derived_body::prepare_derived_body(projected).expect("derived body");
            let derived = handle
                .block_on(fvoci_server::db::collab::project_derived_body_kind(
                    &pool,
                    kind,
                    fvoci_server::db::collab::ProjectDerivedBodyInput::new(
                        source,
                        user,
                        session,
                        target,
                        reloaded.writer_generation,
                        reloaded.load.tail_seq,
                        prepared,
                    ),
                ))
                .unwrap()
                .expect("product derived body");
            assert!(matches!(
                derived,
                fvoci_server::db::collab::ProjectDerivedBodyResult::Updated
                    | fvoci_server::db::collab::ProjectDerivedBodyResult::Unchanged
            ));
        }
        update.expect("one authored update per target")
    })
    .await
    .unwrap()
}

/// Committed native rows of both moved targets, counted as the admission
/// counts them (state + tail updates + receipts + revisions per target).
async fn native_rows(admin: &PgPool, source: Uuid, document: Uuid, task: Uuid) -> (i64, i64, i64) {
    sqlx::query_as(
        r#"SELECT
             (SELECT count(*) FROM fvoci.document_states WHERE workspace_id=$1 AND document_id=$2)
             + (SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id=$1 AND document_id=$2)
             + (SELECT count(*) FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2)
             + (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2)
             + (SELECT count(*) FROM fvoci.task_states WHERE workspace_id=$1 AND task_id=$3)
             + (SELECT count(*) FROM fvoci.task_collab_updates WHERE workspace_id=$1 AND task_id=$3)
             + (SELECT count(*) FROM fvoci.task_collab_op_receipts WHERE workspace_id=$1 AND task_id=$3)
             + (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='task' AND target_id=$3),
           greatest(
             (SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id=$1 AND document_id=$2),
             (SELECT count(*) FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2),
             (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2)),
           greatest(
             (SELECT count(*) FROM fvoci.task_collab_updates WHERE workspace_id=$1 AND task_id=$3),
             (SELECT count(*) FROM fvoci.task_collab_op_receipts WHERE workspace_id=$1 AND task_id=$3),
             (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='task' AND target_id=$3))"#,
    )
    .bind(source)
    .bind(document)
    .bind(task)
    .fetch_one(admin)
    .await
    .unwrap()
}

/// Aggregate copy budget at its DB boundary with genuine data. Setup (not
/// the subject): both targets' combined native rows are driven to exactly
/// MAX_ENTRIES (10 000) with genuine engine updates appended and compacted
/// through the product writer functions as the restricted role, after both
/// rooms are retired, with each target far under the helper's per-target
/// limits and every count observed, never predicted. Subject: the real HTTP
/// preview is admitted at the limit; one more genuine row (a manual task
/// revision of its changed state) makes it 10 001, and both the preview and
/// the commit of the earlier digest are refused as inventory_budget with
/// the complete source and team graphs unchanged.
#[tokio::test]
async fn personal_transfer_move_native_copy_row_budget_boundary() {
    run_test("personal_transfer_move_native_copy_row_budget_boundary", async {
        let t0 = std::time::Instant::now();
        let mark = |phase: &str| eprintln!("[budget-diag +{:.3}s] {phase}", t0.elapsed().as_secs_f64());
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
        mark("setup done");
        let (document, task) = native_pair(addr, &run, &actor, source, &mut selection).await;
        mark("native pair done");
        // No live room may hold a stale copy while rows are written outside it.
        run.hub()
            .retire_moved_resource_rooms(vec![
                fvoci_server::collab::room::RoomKey::document(source, document),
                fvoci_server::collab::room::RoomKey::task(source, task),
            ])
            .await;
        mark("rooms retired");
        let limit = fvoci_server::native_history::MAX_ENTRIES as i64;
        let admin = admin_pool(&run.harness).await;
        let (start, _, _) = native_rows(&admin, source, document, task).await;
        // Bulk to a 32-row margin under the limit; the yield per append is
        // measured, never assumed.
        let bulk = (limit - start - 32).max(0) as usize;
        mark(&format!("start rows {start}, bulk appends {bulk}"));
        // Setup (not product transport/writer evidence): one genuine authored
        // update per target, re-appended under fresh op_ids by the batch.
        let (doc_update, task_update) = tokio::join!(
            drive_native_rows(&run, &actor, source, CollabKind::Document, document, bulk / 2, "문서 행", true, None),
            drive_native_rows(&run, &actor, source, CollabKind::Task, task, bulk - bulk / 2, "작업 행", true, None),
        );
        let (mut rows, _, _) = native_rows(&admin, source, document, task).await;
        mark(&format!("rows after bulk {rows}"));
        assert!(rows <= limit, "bulk overshot: {rows} (start {start}, appends {bulk})");
        // Exact landing through the product append path: the observed
        // remainder in ONE call (one claim, that many genuine product appends,
        // compaction, derived body), then observed again; single appends only
        // if it did not land exactly. Fewer than 64 landing appends in all.
        let mut singles = 0;
        let remainder = (limit - rows) as usize;
        if remainder > 0 {
            assert!(remainder < 64, "landing remainder {remainder} must stay under 64");
            drive_native_rows(&run, &actor, source, CollabKind::Document, document, remainder, "문서 마지막", false, Some(doc_update.clone())).await;
            singles += remainder;
            let (now, _, _) = native_rows(&admin, source, document, task).await;
            // Distinct op_ids with identical bytes are real durable rows.
            assert_eq!(now, rows + remainder as i64, "each product append of the identical update adds one durable row");
            rows = now;
        }
        while rows < limit {
            drive_native_rows(&run, &actor, source, CollabKind::Document, document, 1, "문서 마지막", false, Some(doc_update.clone())).await;
            singles += 1;
            let (now, _, _) = native_rows(&admin, source, document, task).await;
            assert!(now > rows, "a compacted append must add a row ({rows} -> {now})");
            rows = now;
            assert!(singles < 64, "could not land on the limit (rows {rows})");
        }
        let (rows, doc_peak, task_peak) = native_rows(&admin, source, document, task).await;
        mark(&format!("landed rows {rows} after {singles} product appends (doc peak {doc_peak}, task peak {task_peak})"));
        assert_eq!(rows, limit, "exactly at the combined row limit");
        assert!(doc_peak < limit && task_peak < limit, "each target under the per-target helper limit: {doc_peak} {task_peak}");
        // Product append controls at the limit (restricted role): the same
        // op_id with the same bytes is DuplicateAck at its seq, and with other
        // bytes OpIdConflict; neither adds a row.
        {
            use fvoci_server::db::collab::{append_collab_update_kind, claim_writer_and_load_kind, AppendCollabInput, AppendCollabResult, CollabDbError};
            let (op, seq): (Uuid, i64) = sqlx::query_as("SELECT op_id, seq FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2 ORDER BY seq DESC LIMIT 1")
                .bind(source)
                .bind(document)
                .fetch_one(&admin)
                .await
                .unwrap();
            let claim = claim_writer_and_load_kind(&actor.pool, CollabKind::Document, source, actor.user_id, actor.session_id, document).await.unwrap().expect("claim");
            let control = |payload| AppendCollabInput {
                workspace_id: source,
                actor_user_id: actor.user_id,
                session_id: actor.session_id,
                document_id: document,
                writer_generation: claim.writer_generation,
                expected_tail_seq: claim.load.tail_seq,
                op_id: op,
                payload,
                client_ip: None,
            };
            let duplicate = append_collab_update_kind(&actor.pool, CollabKind::Document, control(&doc_update[..])).await.unwrap();
            assert_eq!(duplicate, Ok(AppendCollabResult::DuplicateAck { seq }));
            let conflict = append_collab_update_kind(&actor.pool, CollabKind::Document, control(&task_update[..])).await.unwrap();
            assert_eq!(conflict, Err(CollabDbError::OpIdConflict));
            let (after, _, _) = native_rows(&admin, source, document, task).await;
            assert_eq!(after, limit, "controls add no row");
        }
        selection_versions(&run, source, &mut selection).await;
        let (status, preview) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        mark(&format!("preview at the limit: {status}"));
        assert_eq!(status, StatusCode::OK, "admitted at exactly {limit} rows: {preview}");
        // +1 genuine row: a manual task revision of its changed state.
        let (status, created) = session_call(addr, Method::POST, &task_path(&in_personal(&actor, source), task, "/revisions"), &actor.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let (over, _, _) = native_rows(&admin, source, document, task).await;
        mark(&format!("+1 rows {over}"));
        assert_eq!(over, limit + 1);
        let (status, error) = session_call(addr, Method::POST, &format!("{}/preview", path(source)), &actor.session_token, Some(selection.clone())).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "inventory_budget");
        assert_eq!(error["title"], "native history copy budget");
        let (source_before, team_before) = (transfer_graph(&run, source).await, transfer_graph(&run, actor.workspace_id).await);
        let body = json!({"requestId":Uuid::now_v7(),"selection":selection,"previewDigest":preview["digest"],"confirmed":true});
        let (status, error) = session_call(addr, Method::POST, &path(source), &actor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["params"]["code"], "inventory_budget");
        assert_eq!(transfer_graph(&run, source).await, source_before);
        assert_eq!(transfer_graph(&run, actor.workspace_id).await, team_before);
        mark("refusals checked");
        admin.close().await;
        run.finish().await.unwrap();
    })
    .await;
}
