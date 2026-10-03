//! Real app-role negative gates. These do not establish native restore success.
use super::*;
use axum::http::Request;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tower::ServiceExt;

fn project_only(project: Uuid) -> fvoci_server::db::native_archive::Selection<'static> {
    fvoci_server::db::native_archive::Selection {
        project,
        zotero_connectors: &[],
    }
}

#[tokio::test]
async fn native_archive_delivery_rechecks_captured_resources_after_ordinary_trash() {
    use fvoci_server::db::native_archive::{capture, recheck_delivery, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "ARC",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, document) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents",
            fx.workspace_id
        ),
        Some(json!({"title":"삭제 전 자료 🧪", "parentId":project["rootDocumentId"]})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{document}");
    let document_id = Uuid::parse_str(document["id"].as_str().unwrap()).unwrap();
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    // Raw metadata capture only: this empty-body setup has no independently
    // written native history and is deliberately not a valid archive fixture.
    let captured = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(project_id),
    )
    .await
    .unwrap();
    recheck_delivery(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &captured.archive.graph,
        &captured.file_keys,
    )
    .await
    .unwrap();
    let (status, _) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents/{document_id}/trash",
            fx.workspace_id
        ),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(
        recheck_delivery(
            &fx.pool,
            fx.workspace_id,
            fx.user_id,
            session,
            &captured.archive.graph,
            &captured.file_keys
        )
        .await,
        Err(NativeDbError::Forbidden)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.import_jobs")
            .fetch_one(&fx.admin)
            .await
            .unwrap(),
        0
    );
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

async fn denied_without_reading_body(
    fx: &Fixture,
    workspace: Uuid,
    cookie: Option<&str>,
) -> StatusCode {
    let read = Arc::new(AtomicBool::new(false));
    let observed = read.clone();
    let stream = futures_util::stream::once(async move {
        observed.store(true, Ordering::SeqCst);
        Err::<bytes::Bytes, _>(std::io::Error::other(
            "body must not be read before authorization",
        ))
    });
    let mut request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/workspaces/{workspace}/native-archive/preflight"
        ))
        .header("content-type", "application/json")
        .header("origin", "http://localhost")
        .extension(axum::extract::ConnectInfo(project_harness::test_peer()));
    if let Some(cookie) = cookie {
        // Same session cookie convention as the shared harness requests.
        request = request.header("cookie", format!("fvoci_session={cookie}"));
    }
    let response = fx
        .app
        .clone()
        .oneshot(request.body(Body::from_stream(stream)).unwrap())
        .await
        .unwrap();
    assert!(
        !read.load(Ordering::SeqCst),
        "denied request consumed archive bytes"
    );
    response.status()
}

#[tokio::test]
async fn native_archive_authentication_and_wrong_tenant_precede_body_read() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    assert_eq!(
        denied_without_reading_body(&fx, fx.workspace_id, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        denied_without_reading_body(&fx, Uuid::now_v7(), Some(&fx.cookie)).await,
        StatusCode::FORBIDDEN
    );
    // This ordinary setup workspace is a team: the first private-personal
    // destination adapter must reject it before touching even malformed bytes.
    assert_eq!(
        denied_without_reading_body(&fx, fx.workspace_id, Some(&fx.cookie)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.import_jobs")
            .fetch_one(&fx.admin)
            .await
            .unwrap(),
        0
    );
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_archive_two_installation_sessions_and_roles_are_isolated() {
    let source = TestDb::bootstrap().await;
    let destination = TestDb::bootstrap().await;
    let src = fixture(&source).await;
    let dst = fixture(&destination).await;
    for fx in [&src, &dst] {
        let (superuser, bypass, owner): (bool, bool, bool) = sqlx::query_as(
            "SELECT r.rolsuper,r.rolbypassrls, EXISTS(SELECT 1 FROM pg_tables WHERE schemaname='fvoci' AND tableowner=current_user) FROM pg_roles r WHERE r.rolname=current_user"
        ).fetch_one(&fx.pool).await.unwrap();
        assert_eq!((superuser, bypass, owner), (false, false, false));
    }
    assert_eq!(
        denied_without_reading_body(&dst, dst.workspace_id, Some(&src.cookie)).await,
        StatusCode::UNAUTHORIZED
    );
    let roots = [src.storage_root(), dst.storage_root()];
    src.pool.close().await;
    src.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    source.cleanup().await;
    destination.cleanup().await;
    for root in roots {
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn native_archive_is_not_accepted_by_portable_import_route() {
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let (status, _) = fx
        .import(
            &fx.cookie,
            json!({ "workspaceId":fx.workspace_id,
        "source":"native-archive", "zipBase64":"AA==" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!fvoci_server::import_job::office_format_supported(
        "native-archive",
        false
    ));
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

// ---------------------------------------------------------------------------
// Real app-role publication controls. The archive below is a STRUCTURAL
// fixture (empty native updates): it exercises the single graph/event/job
// transaction, fences and rollback, not native history acceptance, which the
// two-installation browser pair and the collab child tests own.

struct StructuralIds {
    project: Uuid,
    document: Uuid,
    task: Uuid,
    workflow: Uuid,
    status: Uuid,
    activity: Uuid,
    collection: Uuid,
    item: Uuid,
}

impl StructuralIds {
    fn fresh() -> Self {
        Self {
            project: Uuid::now_v7(),
            document: Uuid::now_v7(),
            task: Uuid::now_v7(),
            workflow: Uuid::now_v7(),
            status: Uuid::now_v7(),
            activity: Uuid::now_v7(),
            collection: Uuid::now_v7(),
            item: Uuid::now_v7(),
        }
    }
}

fn structural_archive(ids: &StructuralIds) -> fvoci_server::native_archive::Archive {
    use fvoci_server::native_archive::encode;
    // Independent literals; the source actor/workspace are inert provenance.
    let source_actor = "20000000-0000-4000-8000-000000000001";
    let at = "2026-10-02T00:00:00Z";
    let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"한글 🧪"}]}]});
    let (project, document, task) = (ids.project, ids.document, ids.task);
    let state = |kind: &str, id: Uuid| {
        json!({"target_kind":kind,"target_id":id,
        "state_entry":format!("native/{kind}/{id}/state.v1"),"encoding":1,"snapshot_cutoff_seq":0,
        "tail_seq":0,"compacted_at":null,"created_at":at,"updated_at":at,"updates":[],"receipts":[]})
    };
    let mut archive = json!({
        "graph": {
            "source_workspace_id":"30000000-0000-4000-8000-000000000001","source_actor_id":source_actor,
            "captured_at":at,
            "project":{"id":project,"key":"ARCH","name":"원본 프로젝트 🧪","description":null,"icon":null,
                "visibility":"private","root_document_id":document,"status":"active","next_number":3,
                "created_by":source_actor,"created_at":at,"updated_at":at,"deleted_at":null},
            "workflows":[{"id":ids.workflow,"project_id":project,"created_at":at,"updated_at":at}],
            "statuses":[{"id":ids.status,"project_id":project,"workflow_id":ids.workflow,"name":"진행 전",
                "category":"backlog","sort_key":"a0","wip_limit":null,"created_at":at,"updated_at":at}],
            "documents":[{"id":document,"title":"문서 🧪","icon":null,
                "path":fvoci_server::db::documents::to_path_label(document),"parent_id":null,"sort_key":"a0",
                "project_id":project,"number":1,"status":"draft",
                "schema_version":fvoci_server::db::documents::DOCUMENT_SCHEMA_VERSION,"text":"한글 🧪",
                "chosung":"ㅎㄱ 🧪","version":1,"created_by":source_actor,"created_at":at,"updated_at":at,
                "deleted_at":null,"content_json":body,"kind":"doc"}],
            "tasks":[{"id":task,"project_id":project,"number":2,"title":"일반 태스크 🧪","type":"task",
                "priority":"medium","status_id":ids.status,"start_date":"2026-10-02","due_date":"2026-10-03",
                "due_at":null,"estimate":null,"parent_id":null,"milestone_id":null,"recurrence":null,
                "sort_key":"a0","schema_version":fvoci_server::db::documents::DOCUMENT_SCHEMA_VERSION,
                "content_json":body,"version":1,"archived_at":null,"deleted_at":null,"created_by":source_actor,
                "created_at":at,"updated_at":at,"text":"한글 🧪","chosung":"ㅎㄱ 🧪"}],
            "assignees":[{"task_id":task,"user_id":source_actor}],"labels":[],"task_labels":[],"comments":[],"origins":[],
            "activity":[{"id":ids.activity,"task_id":task,"actor_user_id":source_actor,"channel":"web",
                "kind":"created","changes":[],"created_at":at}],
            "states":[state("document",document),state("task",task)],"revisions":[],"attachments":[],
            "collections":[{"id":ids.collection,"project_id":project,"kind":"task","name":"원본 프로젝트 🧪",
                "version":1,"deleted_at":null,"created_at":"2026-10-02T00:00:01Z","updated_at":"2026-10-02T00:00:02Z"}],
            "collection_items":[{"id":ids.item,"collection_id":ids.collection,"document_id":null,"task_id":task,
                "version":1,"created_at":"2026-10-02T00:00:03Z","updated_at":"2026-10-02T00:00:04Z"}],
            "native_inventory":null
        },
        "entries": {
            format!("native/document/{document}/state.v1"): encode(&[0, 0]),
            format!("native/task/{task}/state.v1"): encode(&[0, 0])
        }
    });
    // The empty model arrays are added outside the literal (one json! of the
    // whole graph exceeds the macro recursion limit).
    for key in [
        "zotero_connectors",
        "zotero_references",
        "zotero_collections",
        "zotero_memberships",
        "zotero_links",
        "personal_input_commands",
        "time_entries",
        "timer_runs",
        "timer_segments",
        "timer_legacy_open",
        "timer_commands",
        "timer_audit",
        "milestones",
        "dependencies",
        "views",
        "document_tags",
        "document_tag_assignments",
        "collection_fields",
        "collection_options",
        "collection_values",
        "collection_choices",
        "collection_people",
        "collection_views",
    ] {
        archive["graph"][key] = json!([]);
    }
    serde_json::from_value(archive).expect("structural archive literal")
}

/// Ordinary personal workspace through the product route, then one durable
/// native command and the worker's claim of it.
async fn claimed_restore(
    fx: &Fixture,
    user: Uuid,
    cookie: &str,
) -> (Uuid, Uuid, fvoci_server::db::import_jobs::ImportClaim) {
    let (status, personal) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {personal}");
    let workspace = Uuid::parse_str(personal["id"].as_str().unwrap()).unwrap();
    let session = project_harness::session_id_for_user(&fx.admin, user).await;
    let payload = b"structural native command payload";
    let hash = fvoci_server::native_archive::digest(payload);
    let request = Uuid::now_v7();
    let job = fvoci_server::db::native_archive::queue_restore(
        &fx.pool, workspace, user, session, request, &hash, payload,
    )
    .await
    .unwrap();
    // Same command after a lost response: the same durable job, not a new one.
    assert_eq!(
        fvoci_server::db::native_archive::queue_restore(
            &fx.pool, workspace, user, session, request, &hash, payload,
        )
        .await
        .unwrap(),
        job
    );
    let changed = b"different bytes for the same command";
    assert!(matches!(
        fvoci_server::db::native_archive::queue_restore(
            &fx.pool,
            workspace,
            user,
            session,
            request,
            &fvoci_server::native_archive::digest(changed),
            changed,
        )
        .await,
        Err(fvoci_server::db::native_archive::NativeDbError::Conflict)
    ));
    let claim = fvoci_server::db::import_jobs::claim_next_import_job(&fx.pool)
        .await
        .unwrap()
        .expect("claim queued native command");
    assert_eq!((claim.job_id, claim.workspace_id), (job, workspace));
    (workspace, session, claim)
}

async fn workspace_graph_counts(admin: &sqlx::PgPool, workspace: Uuid) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "projects",
        "workflows",
        "statuses",
        "documents",
        "tasks",
        "task_assignees",
        "task_activity",
        "document_states",
        "task_states",
        "project_members",
    ] {
        counts.push(
            sqlx::query_scalar::<_, i64>(&format!(
                "SELECT count(*) FROM fvoci.{table} WHERE workspace_id = $1"
            ))
            .bind(workspace)
            .fetch_one(admin)
            .await
            .unwrap(),
        );
    }
    counts
}

async fn job_events(admin: &sqlx::PgPool, workspace: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND (verb = 'native_archive.restored' OR payload ? 'nativeRestoreJobId')",
    )
    .bind(workspace)
    .fetch_one(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn native_publish_commits_mapped_graph_events_and_completion_in_one_transaction() {
    use fvoci_server::db::native_archive::{publish, status};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let ids = StructuralIds::fresh();
    let archive = structural_archive(&ids);
    let cookie = fx.cookie.clone();
    let (workspace, session, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![0; 10]
    );
    publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
    );
    // Content IDs preserved; author/assignee/lead map to the actual actor; the
    // source creation activity keeps its ID with inert (NULL) attribution.
    let (project_creator, assignee, activity_actor, lead): (Uuid, Uuid, Option<Uuid>, Uuid) =
        sqlx::query_as(
            "SELECT p.created_by, a.user_id, act.actor_user_id, m.user_id
             FROM fvoci.projects p
             JOIN fvoci.task_assignees a ON a.task_id = $2
             JOIN fvoci.task_activity act ON act.id = $3
             JOIN fvoci.project_members m ON m.project_id = p.id AND m.role = 'lead'
             WHERE p.id = $1",
        )
        .bind(ids.project)
        .bind(ids.task)
        .bind(ids.activity)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(
        (project_creator, assignee, activity_actor, lead),
        (fx.user_id, fx.user_id, None, fx.user_id)
    );
    let generation: i64 = sqlx::query_scalar(
        "SELECT writer_generation FROM fvoci.document_states WHERE document_id = $1",
    )
    .bind(ids.document)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(generation, 0);
    // The destination's trigger rows now carry the archived baseline identity
    // and timestamps: no duplicate, no fresh UUID.
    let baseline: (i64, i64, Uuid, Uuid, String, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.collections WHERE project_id = $1),
                (SELECT count(*) FROM fvoci.collection_items i JOIN fvoci.collections c
                   ON c.id = i.collection_id WHERE c.project_id = $1),
                (SELECT id FROM fvoci.collections WHERE project_id = $1),
                (SELECT i.id FROM fvoci.collection_items i WHERE i.task_id = $2),
                (SELECT to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM fvoci.collections WHERE project_id = $1),
                (SELECT to_char(i.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM fvoci.collection_items i WHERE i.task_id = $2)",
    )
    .bind(ids.project)
    .bind(ids.task)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(
        baseline,
        (
            1,
            1,
            ids.collection,
            ids.item,
            "2026-10-02 00:00:01".to_string(),
            "2026-10-02 00:00:04".to_string()
        )
    );
    // document.created + task.created + native_archive.restored, all by the actual actor.
    assert_eq!(job_events(&fx.admin, workspace).await, 3);
    let foreign_actor_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND actor_user_id IS DISTINCT FROM $2",
    )
    .bind(workspace)
    .bind(fx.user_id)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(foreign_actor_events, 0);
    let output = status(&fx.pool, workspace, fx.user_id, session, claim.job_id)
        .await
        .unwrap();
    assert_eq!(
        (output.status.as_str(), output.project_id),
        ("completed", Some(ids.project))
    );
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_publish_late_global_collision_rolls_back_entire_graph_and_events() {
    use fvoci_server::db::native_archive::{fail_native, publish, status, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let first = StructuralIds::fresh();
    let cookie = fx.cookie.clone();
    let (_, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &structural_archive(&first),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    // Second actor, second fresh personal workspace in the SAME database: all
    // content IDs fresh except the task activity, inserted after the project,
    // workflow, status, documents, tasks and assignees.
    let other =
        project_harness::add_workspace_user(&fx.admin, fx.workspace_id, "member", "dst2").await;
    let (workspace, session, claim) = claimed_restore(&fx, other.user_id, &other.cookie).await;
    let mut second = StructuralIds::fresh();
    second.activity = first.activity;
    let result = publish(
        &fx.pool,
        &claim,
        &structural_archive(&second),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await;
    assert!(
        matches!(&result, Err(NativeDbError::Sql(error)) if error.as_database_error().is_some_and(|e| e.is_unique_violation())),
        "{result:?}"
    );
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![0; 10]
    );
    assert_eq!(job_events(&fx.admin, workspace).await, 0);
    let running: String = sqlx::query_scalar("SELECT status FROM fvoci.import_jobs WHERE id = $1")
        .bind(claim.job_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(running, "running");
    // The worker's failure transition is generic and terminal.
    assert!(fail_native(&fx.pool, &claim, "conflict").await.unwrap());
    let output = status(&fx.pool, workspace, other.user_id, session, claim.job_id)
        .await
        .unwrap();
    assert_eq!(
        (
            output.status.as_str(),
            output.diagnostic.as_deref(),
            output.project_id
        ),
        ("failed", Some("conflict"), None)
    );
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_publish_refuses_expired_lease_and_revoked_session_without_effects() {
    use fvoci_server::db::native_archive::{publish, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    // Revoke-first: the session is revoked after the claim, before the commit.
    // (Its lease stays live, so the next claim below cannot pick this job.)
    let other =
        project_harness::add_workspace_user(&fx.admin, fx.workspace_id, "member", "revoked").await;
    let (workspace, session, claim) = claimed_restore(&fx, other.user_id, &other.cookie).await;
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE id = $1")
        .bind(session)
        .execute(&fx.admin)
        .await
        .unwrap();
    let revoked = publish(
        &fx.pool,
        &claim,
        &structural_archive(&StructuralIds::fresh()),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await;
    assert!(
        matches!(revoked, Err(NativeDbError::Forbidden)),
        "{revoked:?}"
    );
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![0; 10]
    );
    assert_eq!(job_events(&fx.admin, workspace).await, 0);
    // Lease expiry: a late worker cannot commit after its fence lapsed.
    let cookie = fx.cookie.clone();
    let (workspace, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    sqlx::query(
        "UPDATE fvoci.import_jobs SET lease_until = now() - interval '1 second' WHERE id = $1",
    )
    .bind(claim.job_id)
    .execute(&fx.admin)
    .await
    .unwrap();
    let expired = publish(
        &fx.pool,
        &claim,
        &structural_archive(&StructuralIds::fresh()),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await;
    assert!(matches!(expired, Err(NativeDbError::Fenced)), "{expired:?}");
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![0; 10]
    );
    assert_eq!(job_events(&fx.admin, workspace).await, 0);
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_capture_accepts_baseline_and_person_collection_state_and_refuses_deleted() {
    use fvoci_server::db::native_archive::{capture, NativeDbError};
    use fvoci_server::native_archive::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let refused = |result: Result<_, NativeDbError>| matches!(result, Err(NativeDbError::Archive(ArchiveError::Unsupported(kind))) if kind == "collections");
    // Ordinary project + task: the trigger-created collection/item are baseline.
    let make = |key: &'static str| {
        let app = fx.app.clone();
        let cookie = fx.cookie.clone();
        let ws = fx.workspace_id;
        async move {
            let project =
                project_harness::create_project(app.clone(), &cookie, ws, key, "private").await;
            let id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
            let (status, task) = json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/projects/{id}/tasks"),
                Some(json!({"title":"기준 태스크 🧪"})),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{task}");
            id
        }
    };
    let baseline = make("BASE").await;
    let (collections, items): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.collections WHERE project_id = $1 AND kind = 'task'),
                (SELECT count(*) FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id = i.collection_id WHERE c.project_id = $1)",
    )
    .bind(baseline)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(
        (collections, items),
        (1, 1),
        "observed source automatic baseline"
    );
    let captured = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(baseline),
    )
    .await
    .expect("trigger baseline is capturable");
    let (source_collection, source_items): (Uuid, i64) = sqlx::query_as(
        "SELECT c.id, (SELECT count(*) FROM fvoci.collection_items WHERE collection_id = c.id) FROM fvoci.collections c WHERE c.project_id = $1",
    )
    .bind(baseline)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(
        (
            captured.archive.graph.collections.len(),
            captured.archive.graph.collections[0].id,
            captured.archive.graph.collection_items.len() as i64
        ),
        (1, source_collection, source_items)
    );
    // A person-added field on that same collection is a typed record now
    // (previously the "collections" refusal).
    let (status, collection) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/projects/{baseline}/collection",
            fx.workspace_id
        ),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{collection}");
    let (status, field) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/collections/{}/fields",
            fx.workspace_id,
            collection["id"].as_str().unwrap()
        ),
        Some(json!({"name":"점수","type":"number"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {field}");
    let with_field = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(baseline),
    )
    .await
    .expect("a person-added field is captured");
    assert_eq!(
        with_field
            .archive
            .graph
            .collection_fields
            .iter()
            .map(|f| f.id.to_string())
            .collect::<Vec<_>>(),
        vec![field["id"].as_str().unwrap().to_owned()]
    );
    with_field.archive.validate().unwrap();
    // An additional person-created document collection in the project.
    let extra = make("DOCC").await;
    let (status, created) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/collections", fx.workspace_id),
        Some(json!({"name":"자료 모음","kind":"document","projectId":extra})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {created}");
    let with_documents = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(extra),
    )
    .await
    .expect("a project document collection is captured");
    assert!(with_documents
        .archive
        .graph
        .collections
        .iter()
        .any(|c| c.id.to_string() == created["id"].as_str().unwrap() && c.kind == "document"));
    with_documents.archive.validate().unwrap();
    // Renaming the project leaves the baseline collection's original name,
    // which is now carried as captured (previously a false refusal).
    let renamed = make("RENM").await;
    let (status, patched) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/projects/{renamed}", fx.workspace_id),
        Some(json!({"name":"바뀐 이름 🙂"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(renamed),
    )
    .await
    .expect("a renamed project is captured")
    .archive
    .validate()
    .unwrap();
    // A deleted baseline collection row is also person-changed state.
    let deleted = make("DELC").await;
    sqlx::query("UPDATE fvoci.collections SET deleted_at = now() WHERE project_id = $1")
        .bind(deleted)
        .execute(&fx.admin)
        .await
        .unwrap();
    assert!(refused(
        capture(
            &fx.pool,
            fx.workspace_id,
            fx.user_id,
            session,
            &project_only(deleted)
        )
        .await
    ));
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_publish_baseline_collection_identity_collision_rolls_back() {
    use fvoci_server::db::native_archive::{publish, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let first = StructuralIds::fresh();
    let cookie = fx.cookie.clone();
    let (_, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &structural_archive(&first),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    // All content IDs fresh except the baseline collection UUID.
    let other =
        project_harness::add_workspace_user(&fx.admin, fx.workspace_id, "member", "dstc").await;
    let (workspace, _, claim) = claimed_restore(&fx, other.user_id, &other.cookie).await;
    let mut second = StructuralIds::fresh();
    second.collection = first.collection;
    let result = publish(
        &fx.pool,
        &claim,
        &structural_archive(&second),
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await;
    assert!(
        matches!(&result, Err(NativeDbError::Sql(error)) if error.as_database_error().is_some_and(|e| e.is_unique_violation())),
        "{result:?}"
    );
    assert_eq!(
        workspace_graph_counts(&fx.admin, workspace).await,
        vec![0; 10]
    );
    assert_eq!(job_events(&fx.admin, workspace).await, 0);
    let collections: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.collections WHERE workspace_id = $1")
            .bind(workspace)
            .fetch_one(&fx.admin)
            .await
            .unwrap();
    assert_eq!(collections, 0);
    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

/// One ordinary product write of `body` into the target's native history:
/// writer claim, engine seed update append, engine projection stored as the
/// derived body, then a manual revision of that exact state.
async fn product_native_write(
    fx: &Fixture,
    session: Uuid,
    project: Uuid,
    kind: fvoci_server::collab::wire::CollabKind,
    target: Uuid,
    body: Value,
) -> (Uuid, Uuid) {
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::collab::{
        append_collab_update_kind, claim_writer_and_load_kind, project_derived_body_kind,
        AppendCollabInput, AppendCollabResult, ProjectDerivedBodyInput,
    };
    use fvoci_server::db::revisions::{
        create_manual_revision, CreateRevisionInput, RevisionScope, RevisionTarget,
    };
    let engine = fvoci_server::collab::config::require_collab_engine_for_tests();
    let limits = collab_engine::Limits::default();
    let claim =
        claim_writer_and_load_kind(&fx.pool, kind, fx.workspace_id, fx.user_id, session, target)
            .await
            .unwrap()
            .expect("writer claim");
    let update = fvoci_server::collab::seed::SeedEngine::new(engine.clone(), limits)
        .tiptap_to_yjs_update(&body)
        .await
        .unwrap();
    let op = Uuid::now_v7();
    let seq = match append_collab_update_kind(
        &fx.pool,
        kind,
        AppendCollabInput {
            workspace_id: fx.workspace_id,
            actor_user_id: fx.user_id,
            session_id: session,
            document_id: target,
            writer_generation: claim.writer_generation,
            expected_tail_seq: claim.load.tail_seq,
            op_id: op,
            payload: &update,
            client_ip: None,
        },
    )
    .await
    .unwrap()
    .expect("append")
    {
        AppendCollabResult::Committed { seq } => seq,
        other => panic!("{other:?}"),
    };
    let mut tail: Vec<Vec<u8>> = claim.load.tail.iter().map(|u| u.payload.clone()).collect();
    tail.push(update);
    let captured = fvoci_server::collab::revision::capture_revision_offline(
        engine,
        limits,
        claim.load.snapshot.clone(),
        tail,
    )
    .unwrap();
    let prepared =
        fvoci_server::collab::derived_body::prepare_derived_body(captured.content_json.clone())
            .unwrap();
    let text = prepared.text().to_owned();
    project_derived_body_kind(
        &fx.pool,
        kind,
        ProjectDerivedBodyInput::new(
            fx.workspace_id,
            fx.user_id,
            session,
            target,
            claim.writer_generation,
            seq,
            prepared,
        ),
    )
    .await
    .unwrap()
    .expect("derived body");
    let scope = match kind {
        CollabKind::Document => RevisionScope::project_document(project, target),
        CollabKind::Task => RevisionTarget::Task(target).into(),
    };
    let revision = create_manual_revision(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        scope,
        CreateRevisionInput {
            y_snapshot: captured.y_snapshot,
            content_json: captured.content_json,
            text,
            reason: "manual".into(),
        },
    )
    .await
    .unwrap()
    .expect("manual revision");
    (op, revision)
}

type InventoryResult = Result<
    Option<fvoci_server::db::native_history::NativeHistoryInventory>,
    fvoci_server::db::native_history::NativeDbError,
>;

async fn locked_inventory(
    fx: &Fixture,
    kind: fvoci_server::collab::wire::CollabKind,
    target: Uuid,
    cancel: &CancellationToken,
) -> InventoryResult {
    locked_inventory_on(
        fx.pool.clone(),
        fx.workspace_id,
        fx.user_id,
        kind,
        target,
        cancel,
    )
    .await
}

/// One caller transaction (tenant, inventory, commit) on an owned pool handle
/// so it can run as a spawned task while a test holds conflicting locks.
async fn locked_inventory_on(
    pool: sqlx::PgPool,
    workspace: Uuid,
    actor: Uuid,
    kind: fvoci_server::collab::wire::CollabKind,
    target: Uuid,
    cancel: &CancellationToken,
) -> InventoryResult {
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace)
        .await
        .unwrap();
    let result = fvoci_server::db::native_history::native_history_inventory(
        &mut tx,
        workspace,
        actor,
        kind,
        target,
        &fvoci_server::collab::config::require_collab_engine_for_tests(),
        collab_engine::Limits::default(),
        cancel,
    )
    .await;
    tx.commit().await.unwrap();
    result
}

/// The caller connection's backend identity, for lock witnesses.
async fn backend(conn: &mut sqlx::PgConnection) -> (i32, String) {
    sqlx::query_as("SELECT pg_backend_pid(), current_database()::text")
        .fetch_one(conn)
        .await
        .unwrap()
}

/// Waits until a backend in database `db` running a statement matching `like`
/// is lock-blocked by exactly the `blocker` backend (pg_blocking_pids), and
/// returns that waiter; fails if `finished` reports the waiter completed.
async fn await_blocked(
    admin: &sqlx::PgPool,
    db: &str,
    like: &str,
    blocker: i32,
    finished: impl Fn() -> bool,
) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let waiter: Option<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE datname = $1 AND wait_event_type = 'Lock'
               AND query LIKE $2 AND $3 = ANY(pg_blocking_pids(pid)) LIMIT 1",
        )
        .bind(db)
        .bind(like)
        .bind(blocker)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(waiter) = waiter {
            assert_ne!(waiter, blocker);
            return waiter;
        }
        assert!(!finished(), "the waiter passed a held row lock");
        assert!(
            tokio::time::Instant::now() < deadline,
            "the waiter never blocked on the holder's row lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Every revision row of a target with exact payload bytes, oldest first.
async fn revision_rows(admin: &sqlx::PgPool, target: Uuid) -> Vec<Value> {
    sqlx::query_scalar(
        "SELECT to_jsonb(r) - 'y_snapshot' || jsonb_build_object('y_snapshot', encode(r.y_snapshot, 'hex'))
           FROM fvoci.revisions r WHERE target_id = $1 ORDER BY created_at, id",
    )
    .bind(target)
    .fetch_all(admin)
    .await
    .unwrap()
}

fn without(rows: &[Value], removed: &[Uuid]) -> Vec<Value> {
    rows.iter()
        .filter(|row| {
            !removed
                .iter()
                .any(|id| row["id"].as_str() == Some(id.to_string().as_str()))
        })
        .cloned()
        .collect()
}

/// The same caller transaction with an explicit engine path.
async fn locked_inventory_with(
    fx: &Fixture,
    kind: fvoci_server::collab::wire::CollabKind,
    target: Uuid,
    engine: &std::path::Path,
) -> InventoryResult {
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    let result = fvoci_server::db::native_history::native_history_inventory(
        &mut tx,
        fx.workspace_id,
        fx.user_id,
        kind,
        target,
        engine,
        collab_engine::Limits::default(),
        &CancellationToken::new(),
    )
    .await;
    tx.commit().await.unwrap();
    result
}

async fn native_rows(admin: &sqlx::PgPool, table: &str, target: Uuid) -> Value {
    sqlx::query_scalar(&format!(
        "SELECT jsonb_build_array(
            (SELECT to_jsonb(s)-'state' || jsonb_build_object('state',encode(s.state,'hex')) FROM fvoci.{table}_states s WHERE {table}_id=$1),
            (SELECT count(*) FROM fvoci.{table}_collab_updates WHERE {table}_id=$1),
            (SELECT count(*) FROM fvoci.{table}_collab_op_receipts WHERE {table}_id=$1),
            (SELECT count(*) FROM fvoci.revisions WHERE target_id=$1))"
    ))
    .bind(target)
    .fetch_one(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn native_history_inventory_reads_locked_canonical_history_and_types_blockers() {
    use collab_engine::archive_history::{ReferenceCertainty, ReferenceKind};
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::native_history::NativeDbError;
    use fvoci_server::native_history::{ArchiveError, RetainedHistoryBlocker};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let cancel = CancellationToken::new();
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "INV",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let root = Uuid::parse_str(project["rootDocumentId"].as_str().unwrap()).unwrap();
    let new_document = |title: &'static str| {
        let (app, cookie, ws, parent) = (
            fx.app.clone(),
            fx.cookie.clone(),
            fx.workspace_id,
            root.to_string(),
        );
        async move {
            let (status, document) = json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents"),
                Some(json!({"title":title, "parentId":parent})),
                Some(&cookie),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{document}");
            Uuid::parse_str(document["id"].as_str().unwrap()).unwrap()
        }
    };
    let document = new_document("기록 문서 🧪").await;
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/tasks",
            fx.workspace_id
        ),
        Some(json!({"title":"기록 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();

    // Never-native targets: the stored body is the whole body.
    for (kind, id) in [(CollabKind::Document, document), (CollabKind::Task, task)] {
        assert!(locked_inventory(&fx, kind, id, &cancel)
            .await
            .unwrap()
            .is_none());
    }
    // A target outside the caller's tenant is not read.
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    assert!(matches!(
        fvoci_server::db::native_history::native_history_inventory(
            &mut tx,
            Uuid::now_v7(),
            fx.user_id,
            CollabKind::Document,
            document,
            &fvoci_server::collab::config::require_collab_engine_for_tests(),
            collab_engine::Limits::default(),
            &cancel,
        )
        .await,
        Err(NativeDbError::Forbidden)
    ));
    tx.rollback().await.unwrap();

    let mention = |id: Uuid| {
        json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"text","text":"앞 🧪 "},
            {"type":"mention","attrs":{"entity":"document","id":id,"label":"루트"}},
            {"type":"text","text":" 외부","marks":[{"type":"link","attrs":{"href":"https://example.org/api/v2/a"}}]}
        ]}]})
    };
    let (document_op, document_revision) = product_native_write(
        &fx,
        session,
        project_id,
        CollabKind::Document,
        document,
        mention(root),
    )
    .await;
    let (task_op, task_revision) = product_native_write(
        &fx,
        session,
        project_id,
        CollabKind::Task,
        task,
        mention(document),
    )
    .await;
    for (kind, table, id, op, revision, referenced) in [
        (
            CollabKind::Document,
            "document",
            document,
            document_op,
            document_revision,
            root,
        ),
        (
            CollabKind::Task,
            "task",
            task,
            task_op,
            task_revision,
            document,
        ),
    ] {
        let before = native_rows(&fx.admin, table, id).await;
        let inventory = locked_inventory(&fx, kind, id, &cancel)
            .await
            .unwrap()
            .expect("native history");
        // Read-only: the committed caller transaction changed no native row.
        assert_eq!(native_rows(&fx.admin, table, id).await, before);
        assert_eq!(inventory.target_id, id);
        assert_eq!((inventory.snapshot_cutoff_seq, inventory.tail_seq), (0, 1));
        assert_eq!(inventory.tail, vec![(1, op)]);
        assert_eq!(inventory.receipts, vec![(op, 1, fx.user_id)]);
        assert_eq!(inventory.revisions, vec![revision]);
        assert_eq!(inventory.binding.len(), 64);
        assert_eq!(inventory.report.binding, inventory.binding);
        assert!(inventory.report.complete && inventory.report.diagnostics.is_empty());
        assert!(inventory.report.unavailable.is_empty());
        let typed: Vec<_> = inventory
            .report
            .references
            .iter()
            .filter(|r| r.kind != ReferenceKind::LinkHref)
            .map(|r| (r.kind, r.certainty, r.value.clone()))
            .collect();
        assert_eq!(
            typed,
            vec![(
                ReferenceKind::Document,
                ReferenceCertainty::FixedKind,
                referenced.to_string()
            )]
        );
        assert_eq!(
            inventory.closure_blocker(|k, v| k == ReferenceKind::Document && v == referenced),
            None
        );
        assert_eq!(
            inventory.closure_blocker(|_, _| false),
            Some(RetainedHistoryBlocker::OutsideClosure)
        );
    }

    // Application links in retained history are typed blockers.
    let linked = new_document("앱 링크 🧪").await;
    product_native_write(
        &fx,
        session,
        project_id,
        CollabKind::Document,
        linked,
        json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"앱",
            "marks":[{"type":"link","attrs":{"href":format!("/api/v1/workspaces/{}/documents/{root}", fx.workspace_id)}}]}]}]}),
    )
    .await;
    let inventory = locked_inventory(&fx, CollabKind::Document, linked, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        inventory.closure_blocker(|_, _| true),
        Some(RetainedHistoryBlocker::ApplicationLink)
    );

    // The caller's lock holds the cut: a concurrent writer waits on the state
    // row until the caller transaction ends.
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    fvoci_server::db::native_history::native_history_inventory(
        &mut tx,
        fx.workspace_id,
        fx.user_id,
        CollabKind::Document,
        document,
        &fvoci_server::collab::config::require_collab_engine_for_tests(),
        collab_engine::Limits::default(),
        &cancel,
    )
    .await
    .unwrap()
    .unwrap();
    let engine = fvoci_server::collab::config::require_collab_engine_for_tests();
    let update = fvoci_server::collab::seed::SeedEngine::new(engine, collab_engine::Limits::default())
        .tiptap_to_yjs_update(&json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"동시"}]}]}))
        .await
        .unwrap();
    let generation: i64 = sqlx::query_scalar(
        "SELECT writer_generation FROM fvoci.document_states WHERE document_id=$1",
    )
    .bind(document)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    let (pool, ws, user) = (fx.pool.clone(), fx.workspace_id, fx.user_id);
    let append = tokio::spawn(async move {
        fvoci_server::db::collab::append_collab_update(
            &pool,
            fvoci_server::db::collab::AppendCollabInput {
                workspace_id: ws,
                actor_user_id: user,
                session_id: session,
                document_id: document,
                writer_generation: generation,
                expected_tail_seq: 1,
                op_id: Uuid::now_v7(),
                payload: &update,
                client_ip: None,
            },
        )
        .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock' AND query LIKE '%document_states%'",
        )
        .fetch_one(&fx.admin)
        .await
        .unwrap();
        if waiting > 0 {
            break;
        }
        assert!(
            !append.is_finished(),
            "writer passed the caller's state lock"
        );
        assert!(
            tokio::time::Instant::now() < deadline,
            "writer never waited on the state lock"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tx.rollback().await.unwrap();
    assert!(matches!(
        append.await.unwrap().unwrap(),
        Ok(fvoci_server::db::collab::AppendCollabResult::Committed { seq: 2 })
    ));
    // The stored body no longer equals the native state: refused, not guessed.
    assert!(matches!(
        locked_inventory(&fx, CollabKind::Document, document, &cancel).await,
        Err(NativeDbError::Archive(ArchiveError::Invalid(m))) if m == "native/body disagreement"
    ));

    // A receipt that no longer matches its retained operation is refused.
    sqlx::query(
        "UPDATE fvoci.task_collab_op_receipts SET payload_len = payload_len + 1 WHERE task_id = $1",
    )
    .bind(task)
    .execute(&fx.admin)
    .await
    .unwrap();
    assert!(matches!(
        locked_inventory(&fx, CollabKind::Task, task, &cancel).await,
        Err(NativeDbError::Archive(ArchiveError::Invalid(m))) if m == "native history continuity"
    ));
    // Cancellation before the child runs is a typed stop, not a result.
    let stopped = CancellationToken::new();
    stopped.cancel();
    assert!(matches!(
        locked_inventory(&fx, CollabKind::Document, linked, &stopped).await,
        Err(NativeDbError::Archive(ArchiveError::Cancelled))
    ));

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

/// Copies of one real revision (same native snapshot and projected JSON)
/// under new IDs, ordered after it.
async fn copy_revision(admin: &sqlx::PgPool, source: Uuid, reason: &str, copies: i32) -> Vec<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO fvoci.revisions SELECT (jsonb_populate_record(NULL::fvoci.revisions,
            to_jsonb(r) || jsonb_build_object('id', gen_random_uuid(), 'reason', $2::text,
              'created_at', r.created_at + make_interval(secs => g)))).*
         FROM fvoci.revisions r, generate_series(1, $3) g WHERE r.id = $1
         RETURNING id",
    )
    .bind(source)
    .bind(reason)
    .bind(copies)
    .fetch_all(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn native_history_inventory_holds_revision_rows_against_gc() {
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::native_history::NativeDbError;
    use fvoci_server::native_history::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let cancel = CancellationToken::new();
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "GCB",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, document) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents",
            fx.workspace_id
        ),
        Some(json!({"title":"보존 문서 🧪", "parentId":project["rootDocumentId"]})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{document}");
    let document = Uuid::parse_str(document["id"].as_str().unwrap()).unwrap();
    let (_, manual) = product_native_write(
        &fx,
        session,
        project_id,
        CollabKind::Document,
        document,
        json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"기록 🧪"}]}]}),
    )
    .await;
    let automatic = copy_revision(&fx.admin, manual, "session", 2).await;
    let first = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.revisions, [vec![manual], automatic.clone()].concat());

    let engine = fvoci_server::collab::config::require_collab_engine_for_tests();
    let gc_task = |pool: sqlx::PgPool, ws: Uuid| {
        tokio::spawn(async move {
            fvoci_server::db::revisions::gc_automatic_revisions_batch(&pool, ws, 0, 100).await
        })
    };

    // Automatic-revision GC takes only its own row locks. While the caller's
    // transaction holds the inventoried rows, GC is blocked by exactly that
    // backend; it deletes them only after the caller commits, and every
    // surviving row keeps its exact bytes.
    let before = revision_rows(&fx.admin, document).await;
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    let (caller, db) = backend(&mut tx).await;
    let held = fvoci_server::db::native_history::native_history_inventory(
        &mut tx,
        fx.workspace_id,
        fx.user_id,
        CollabKind::Document,
        document,
        &engine,
        collab_engine::Limits::default(),
        &cancel,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(held.revisions, first.revisions);
    assert_eq!(held.content_digest, first.content_digest);
    assert_ne!(held.binding, first.binding);
    let gc = gc_task(fx.pool.clone(), fx.workspace_id);
    await_blocked(&fx.admin, &db, "%FROM fvoci.revisions%", caller, || {
        gc.is_finished()
    })
    .await;
    assert_eq!(revision_rows(&fx.admin, document).await, before);
    tx.commit().await.unwrap();
    assert_eq!(gc.await.unwrap().unwrap(), 2);
    assert_eq!(
        revision_rows(&fx.admin, document).await,
        without(&before, &automatic)
    );
    let after = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.revisions, vec![manual]);
    assert_ne!(after.content_digest, first.content_digest);

    // A cancelled inventory still took its locks; GC stays blocked by the
    // caller until it rolls back, then proceeds.
    let automatic = copy_revision(&fx.admin, manual, "session", 2).await;
    let before = revision_rows(&fx.admin, document).await;
    let stopped = CancellationToken::new();
    stopped.cancel();
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    let (caller, db) = backend(&mut tx).await;
    assert!(matches!(
        fvoci_server::db::native_history::native_history_inventory(
            &mut tx,
            fx.workspace_id,
            fx.user_id,
            CollabKind::Document,
            document,
            &engine,
            collab_engine::Limits::default(),
            &stopped,
        )
        .await,
        Err(NativeDbError::Archive(ArchiveError::Cancelled))
    ));
    let gc = gc_task(fx.pool.clone(), fx.workspace_id);
    await_blocked(&fx.admin, &db, "%FROM fvoci.revisions%", caller, || {
        gc.is_finished()
    })
    .await;
    assert_eq!(revision_rows(&fx.admin, document).await, before);
    tx.rollback().await.unwrap();
    assert_eq!(gc.await.unwrap().unwrap(), 2);
    assert_eq!(
        revision_rows(&fx.admin, document).await,
        without(&before, &automatic)
    );

    // GC first: the inventory is blocked by exactly the deleter's backend. A
    // rolled-back delete leaves the rows in the inventory; a committed delete
    // removes them, the remaining history still replays and survivors keep
    // their bytes.
    let automatic = copy_revision(&fx.admin, manual, "session", 2).await;
    let before = revision_rows(&fx.admin, document).await;
    for commit in [false, true] {
        let mut deleter = fx.pool.begin().await.unwrap();
        fvoci_server::db::context::set_tenant(&mut deleter, fx.workspace_id)
            .await
            .unwrap();
        let (holder, db) = backend(&mut deleter).await;
        let deleted = sqlx::query(
            "DELETE FROM fvoci.revisions WHERE workspace_id = $1 AND id = ANY($2) AND reason IN ('session', 'scheduled')",
        )
        .bind(fx.workspace_id)
        .bind(&automatic)
        .execute(&mut *deleter)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(deleted, 2);
        let (pool, ws, user) = (fx.pool.clone(), fx.workspace_id, fx.user_id);
        let inventory = tokio::spawn(async move {
            locked_inventory_on(
                pool,
                ws,
                user,
                CollabKind::Document,
                document,
                &CancellationToken::new(),
            )
            .await
        });
        await_blocked(
            &fx.admin,
            &db,
            "%FROM fvoci.revisions%FOR UPDATE%",
            holder,
            || inventory.is_finished(),
        )
        .await;
        let (expected, rows) = if commit {
            deleter.commit().await.unwrap();
            (vec![manual], without(&before, &automatic))
        } else {
            deleter.rollback().await.unwrap();
            ([vec![manual], automatic.clone()].concat(), before.clone())
        };
        let inventory = inventory.await.unwrap().unwrap().unwrap();
        assert_eq!(inventory.revisions, expected);
        assert!(inventory.report.complete);
        assert_eq!(revision_rows(&fx.admin, document).await, rows);
    }

    // Real manual-revision promotion of the newest automatic revision is a
    // metadata update on an inventoried row: it is blocked by the caller,
    // happens only after the caller commits, and changes the content digest
    // while the revision IDs and payload bytes stay the same.
    let promoted = copy_revision(&fx.admin, manual, "session", 1).await[0];
    let before = revision_rows(&fx.admin, document).await;
    let mut tx = fx.pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
        .await
        .unwrap();
    let (caller, db) = backend(&mut tx).await;
    let held = fvoci_server::db::native_history::native_history_inventory(
        &mut tx,
        fx.workspace_id,
        fx.user_id,
        CollabKind::Document,
        document,
        &engine,
        collab_engine::Limits::default(),
        &cancel,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(held.revisions, vec![manual, promoted]);
    let (y_snapshot, content_json, text): (Vec<u8>, Value, String) =
        sqlx::query_as("SELECT y_snapshot, content_json, text FROM fvoci.revisions WHERE id = $1")
            .bind(promoted)
            .fetch_one(&fx.admin)
            .await
            .unwrap();
    let (pool, ws, user) = (fx.pool.clone(), fx.workspace_id, fx.user_id);
    let promote = tokio::spawn(async move {
        fvoci_server::db::revisions::create_manual_revision(
            &pool,
            ws,
            user,
            session,
            fvoci_server::db::revisions::RevisionScope::project_document(project_id, document),
            fvoci_server::db::revisions::CreateRevisionInput {
                y_snapshot,
                content_json,
                text,
                reason: "manual".into(),
            },
        )
        .await
    });
    await_blocked(
        &fx.admin,
        &db,
        "%FROM fvoci.revisions%FOR UPDATE%",
        caller,
        || promote.is_finished(),
    )
    .await;
    assert_eq!(revision_rows(&fx.admin, document).await, before);
    tx.commit().await.unwrap();
    assert_eq!(promote.await.unwrap().unwrap().unwrap(), promoted);
    let rows = revision_rows(&fx.admin, document).await;
    assert_eq!(rows.len(), before.len());
    for (old, new) in before.iter().zip(&rows) {
        let mut expected = old.clone();
        if old["id"].as_str() == Some(promoted.to_string().as_str()) {
            expected["reason"] = json!("manual");
            expected["created_by"] = json!(fx.user_id);
        }
        assert_eq!(new, &expected);
    }
    let after = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.revisions, held.revisions);
    assert_ne!(after.content_digest, held.content_digest);

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

/// A private project document holding one ordinary product write of
/// `paragraphs` x `chars`-byte paragraphs and its manual revision.
async fn written_document(
    fx: &Fixture,
    session: Uuid,
    key: &str,
    paragraphs: usize,
    chars: usize,
) -> (Uuid, Uuid) {
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        key,
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, document) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents",
            fx.workspace_id
        ),
        Some(json!({"title":"큰 기록 🧪", "parentId":project["rootDocumentId"]})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{document}");
    let document = Uuid::parse_str(document["id"].as_str().unwrap()).unwrap();
    let content: Vec<Value> = (0..paragraphs)
        .map(|i| json!({"type":"paragraph","content":[{"type":"text","text":format!("{i:04}{}", "a".repeat(chars - 4))}]}))
        .collect();
    let (_, manual) = product_native_write(
        fx,
        session,
        project_id,
        fvoci_server::collab::wire::CollabKind::Document,
        document,
        json!({"type":"doc","content":content}),
    )
    .await;
    (document, manual)
}

/// Each isolated child step the inventory takes for one stored target, with
/// the exact engine status: archive load (and its report bounds), projection,
/// revision load, restore, apply and projection.
async fn engine_stages(
    admin: &sqlx::PgPool,
    document: Uuid,
    revision: Uuid,
) -> Vec<(&'static str, collab_engine::outcome::EngineStatus)> {
    let state: Vec<u8> =
        sqlx::query_scalar("SELECT state FROM fvoci.document_states WHERE document_id = $1")
            .bind(document)
            .fetch_one(admin)
            .await
            .unwrap();
    let tail: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT u.payload FROM fvoci.document_collab_updates u JOIN fvoci.document_states s
           ON s.document_id = u.document_id
          WHERE u.document_id = $1 AND u.seq > s.snapshot_cutoff_seq AND u.seq <= s.tail_seq
          ORDER BY u.seq",
    )
    .bind(document)
    .fetch_all(admin)
    .await
    .unwrap();
    let y_snapshot: Vec<u8> =
        sqlx::query_scalar("SELECT y_snapshot FROM fvoci.revisions WHERE id = $1")
            .bind(revision)
            .fetch_one(admin)
            .await
            .unwrap();
    let engine = fvoci_server::collab::config::require_collab_engine_for_tests();
    tokio::task::spawn_blocking(move || {
        use collab_engine::{
            outcome::EngineStatus,
            process::{ChildSlotKind, EngineSession, SpawnRequest},
            protocol::Request,
        };
        let limits = collab_engine::Limits::default();
        let spawn = || {
            EngineSession::spawn(SpawnRequest {
                engine_bin: engine.clone(),
                limits,
                slot_kind: ChildSlotKind::Primary,
                slot_wait: None,
                test_hang_ms: None,
                test_exit_after_read: None,
                test_close_stdout_hang_ms: None,
                test_exit_after_write: None,
            })
            .unwrap()
        };
        let binding = "0".repeat(64);
        let mut stages = Vec::new();
        let mut child = spawn();
        let load = child
            .call(&Request::ArchiveLoad {
                snapshot_b64: Some(state.clone()),
                tail_b64: tail.clone(),
                encoding: 1,
                capture_binding: binding.clone(),
            })
            .outcome;
        if let EngineStatus::Ok {
            native_archive_inventory: Some(report),
            ..
        } = &load
        {
            if let Err(error) =
                fvoci_server::native_history::check_report_bounds(report, &binding, &limits)
            {
                stages.push((
                    "report-bounds",
                    EngineStatus::Malformed {
                        detail: format!("{error:?} work={:?}", report.work),
                    },
                ));
            }
        }
        let loaded = matches!(load, EngineStatus::Ok { .. });
        stages.push(("archive-load", load));
        if loaded {
            stages.push((
                "project",
                child.call(&Request::Project { encoding: 1 }).outcome,
            ));
        }
        let mut child = spawn();
        stages.push((
            "revision-load",
            child
                .call(&Request::Load {
                    snapshot_b64: Some(state),
                    tail_b64: tail,
                    encoding: 1,
                })
                .outcome,
        ));
        let restore = child
            .call(&Request::ArchiveRestoreFromSnapshot {
                snap_b64: y_snapshot,
                encoding: 1,
            })
            .outcome;
        let update = match &restore {
            EngineStatus::Ok {
                update_b64: Some(bytes),
                ..
            } => Some(fvoci_server::native_history::decode(bytes).unwrap()),
            _ => None,
        };
        stages.push(("restore", restore));
        if let Some(update) = update {
            stages.push((
                "apply",
                child
                    .call(&Request::Apply {
                        update_b64: update,
                        encoding: 1,
                    })
                    .outcome,
            ));
            stages.push((
                "revision-project",
                child.call(&Request::Project { encoding: 1 }).outcome,
            ));
        }
        stages
    })
    .await
    .unwrap()
}

fn engine_budget_refusal(status: &collab_engine::outcome::EngineStatus) -> bool {
    matches!(status, collab_engine::outcome::EngineStatus::ResourceLimit { detail, .. }
        if detail == "native archive inventory budget")
}

#[tokio::test]
async fn native_history_inventory_refuses_near_cap_document_at_engine_budget() {
    use collab_engine::outcome::EngineStatus;
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::native_history::NativeDbError;
    use fvoci_server::native_history::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    // The original corpus: one ordinary product write of 850 x 1000-byte
    // paragraphs (revision JSON within the product body cap). The engine's
    // archive inventory and restore proofs exceed their existing budgets for
    // it; that refusal is the explicit product limitation, not an omission.
    let (document, manual) = written_document(&fx, session, "CAP", 850, 1000).await;
    let cap = fvoci_server::collab::derived_body::DOCUMENT_MAX_BODY_BYTES as i64;
    let revision_json: i64 = sqlx::query_scalar(
        "SELECT octet_length(content_json::text)::bigint FROM fvoci.revisions WHERE id = $1",
    )
    .bind(manual)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert!(
        revision_json > cap * 3 / 4 && revision_json <= cap,
        "{revision_json}"
    );
    let stages = engine_stages(&fx.admin, document, manual).await;
    let status = |name: &str| {
        stages
            .iter()
            .find(|(stage, _)| *stage == name)
            .map(|(_, status)| status)
    };
    // The archive load is refused by an engine inventory budget (the measured
    // corpus exceeds both the 32 MiB memory and 100k step ledgers; which one
    // trips first is in the status recorded by the log).
    assert!(
        status("archive-load").is_some_and(engine_budget_refusal),
        "{stages:?}"
    );
    println!("near-cap archive-load status: {:?}", status("archive-load"));
    assert!(
        matches!(status("revision-load"), Some(EngineStatus::Ok { .. })),
        "{stages:?}"
    );
    // Restore proofs refuse on the step ledger (~127k steps measured).
    assert!(
        matches!(status("restore"), Some(EngineStatus::ResourceLimit { kind: collab_engine::outcome::LimitKind::Ops, detail })
            if detail == "native archive inventory budget"),
        "{stages:?}"
    );
    // The parent's admission precharge passes (an unavailable engine is
    // reached), and the real engine refuses with a typed Limit.
    let missing = std::path::PathBuf::from("/nonexistent/fvoci-collab-engine");
    let reached = locked_inventory_with(&fx, CollabKind::Document, document, &missing).await;
    assert!(
        matches!(reached, Err(NativeDbError::Archive(ArchiveError::Worker))),
        "{reached:?}"
    );
    let refused = locked_inventory(
        &fx,
        CollabKind::Document,
        document,
        &CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(refused, Err(NativeDbError::Archive(ArchiveError::Limit))),
        "{refused:?}"
    );

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_history_inventory_bounds_coherent_history_json_before_reading() {
    use collab_engine::outcome::EngineStatus;
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::native_history::NativeDbError;
    use fvoci_server::native_history::{ArchiveError, MAX_GRAPH_BYTES};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let cancel = CancellationToken::new();
    // A lower-complexity near-body-cap document (32 x 27000-byte paragraphs,
    // far fewer native items than the 850-paragraph original): the aggregate
    // JSON admission is exercised with revisions that are each exactly this
    // native snapshot and its projected JSON.
    let (document, manual) = written_document(&fx, session, "AGG", 32, 27000).await;
    let cap = fvoci_server::collab::derived_body::DOCUMENT_MAX_BODY_BYTES as i64;
    let (body_json, revision_json): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT octet_length(content_json::text)::bigint FROM fvoci.documents WHERE id = $1),
                (SELECT octet_length(content_json::text)::bigint FROM fvoci.revisions WHERE id = $2)",
    )
    .bind(document)
    .bind(manual)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    for json in [body_json, revision_json] {
        assert!(
            json > cap * 3 / 4 && json <= cap,
            "{body_json} {revision_json}"
        );
    }
    let stages = engine_stages(&fx.admin, document, manual).await;
    assert!(
        stages
            .iter()
            .all(|(_, status)| matches!(status, EngineStatus::Ok { .. })),
        "{:?}",
        stages
            .iter()
            .filter(|(_, status)| !matches!(status, EngineStatus::Ok { .. }))
            .collect::<Vec<_>>()
    );
    let (body_bytes, revision_bytes): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT octet_length(content_json::text)::bigint FROM fvoci.documents WHERE id = $1),
                (SELECT (octet_length(content_json::text) + octet_length(text))::bigint FROM fvoci.revisions WHERE id = $2)",
    )
    .bind(document)
    .bind(manual)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    let limit = MAX_GRAPH_BYTES as i64;
    let fit = (limit - body_bytes - revision_bytes) / revision_bytes;
    assert!(fit >= 1);
    copy_revision(&fx.admin, manual, "manual", fit as i32).await;
    assert!(body_bytes + revision_bytes * (1 + fit) <= limit);
    // Discriminator: at the budget an unavailable engine is reached (Worker),
    // so the admission precharge passed; a Limit below can only come from it.
    let missing = std::path::PathBuf::from("/nonexistent/fvoci-collab-engine");
    let reached = locked_inventory_with(&fx, CollabKind::Document, document, &missing).await;
    assert!(
        matches!(reached, Err(NativeDbError::Archive(ArchiveError::Worker))),
        "{reached:?}"
    );
    // Green: the whole coherent corpus at the budget replays.
    let inventory = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inventory.revisions.len() as i64, 1 + fit);
    assert!(inventory.report.complete);
    copy_revision(&fx.admin, manual, "manual", 1).await;
    assert!(body_bytes + revision_bytes * (2 + fit) > limit);
    // Red: one more coherent revision is refused by the budget before any
    // body/revision JSON or native payload is read (also with no engine).
    assert!(matches!(
        locked_inventory(&fx, CollabKind::Document, document, &cancel).await,
        Err(NativeDbError::Archive(ArchiveError::Limit))
    ));
    assert!(matches!(
        locked_inventory_with(&fx, CollabKind::Document, document, &missing).await,
        Err(NativeDbError::Archive(ArchiveError::Limit))
    ));

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_captures_and_publishes_labels_and_changed_activity() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    // Ordinary product flow: a project label, a task, then a task patch that
    // assigns the label and the actor (one "changed" activity).
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "LBL",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, label) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/labels",
            fx.workspace_id
        ),
        Some(json!({"name":"검토 🧪","color":"teal"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{label}");
    let label_id = Uuid::parse_str(label["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/tasks",
            fx.workspace_id
        ),
        Some(json!({"title":"라벨 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let (status, patched) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/tasks/{task_id}", fx.workspace_id),
        Some(json!({"labelIds":[label_id],"assigneeIds":[fx.user_id]})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    let captured = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &project_only(project_id),
    )
    .await
    .expect("labels are captured, not refused");
    let graph = &captured.archive.graph;
    assert_eq!(graph.labels.len(), 1);
    assert_eq!(
        (
            graph.labels[0].id,
            graph.labels[0].project_id,
            graph.labels[0].name.as_str(),
            graph.labels[0].color.as_str()
        ),
        (label_id, project_id, "검토 🧪", "teal")
    );
    assert_eq!(graph.task_labels.len(), 1);
    assert_eq!(
        (graph.task_labels[0].task_id, graph.task_labels[0].label_id),
        (task_id, label_id)
    );
    let changed: Vec<_> = graph
        .activity
        .iter()
        .filter(|a| a.task_id == task_id && a.kind == "changed")
        .collect();
    assert_eq!(changed.len(), 1);
    // The stored snapshot form (db::task_activity): reference objects, the
    // label's name kept as history and the assignee label null.
    assert_eq!(
        changed[0].changes,
        json!([
            {"field":"assigneeIds","from":[],"to":[{"id": fx.user_id.to_string(), "label": null}]},
            {"field":"labelIds","from":[],"to":[{"id": label_id.to_string(), "label": "검토 🧪"}]}
        ])
    );

    // Publish of a structural archive carrying a label, its assignment and a
    // changed activity naming the source actor: same label/activity IDs, the
    // assignee identity inside the activity becomes the destination actor.
    let ids = StructuralIds::fresh();
    let mut archive = structural_archive(&ids);
    let source_actor = archive.graph.source_actor_id;
    let restored_label = Uuid::now_v7();
    let changed_activity = Uuid::now_v7();
    archive.graph.labels = vec![serde_json::from_value(json!({
        "id": restored_label, "project_id": ids.project, "name": "검토 🧪", "color": "teal",
        "created_at": "2026-10-02T00:00:05Z", "updated_at": "2026-10-02T00:00:06Z"
    }))
    .unwrap()];
    archive.graph.task_labels =
        vec![
            serde_json::from_value(json!({"task_id": ids.task, "label_id": restored_label}))
                .unwrap(),
        ];
    archive.graph.activity.push(
        serde_json::from_value(json!({
            "id": changed_activity, "task_id": ids.task, "actor_user_id": source_actor,
            "channel": "web", "kind": "changed", "created_at": "2026-10-02T00:00:07Z",
            "changes": [{"field":"labelIds","from":[],"to":[{"id": restored_label, "label": "검토 🧪"}]},
                        {"field":"assigneeIds","from":[],"to":[{"id": source_actor, "label": null}]}]
        }))
        .unwrap(),
    );
    archive
        .validate()
        .expect("structural archive with labels is valid");
    let cookie = fx.cookie.clone();
    let (workspace, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    let (label_row, assignment, changes, actor): (Value, i64, Value, Option<Uuid>) =
        sqlx::query_as(
            "SELECT (SELECT to_jsonb(l) - 'workspace_id' - 'created_at' - 'updated_at' FROM fvoci.labels l WHERE l.id = $2),
                    (SELECT count(*) FROM fvoci.task_labels WHERE workspace_id = $1 AND task_id = $3 AND label_id = $2),
                    (SELECT changes FROM fvoci.task_activity WHERE id = $4),
                    (SELECT actor_user_id FROM fvoci.task_activity WHERE id = $4)",
        )
        .bind(workspace)
        .bind(restored_label)
        .bind(ids.task)
        .bind(changed_activity)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(
        label_row,
        json!({"id": restored_label, "project_id": ids.project, "name": "검토 🧪", "color": "teal"})
    );
    assert_eq!(assignment, 1);
    assert_eq!(
        changes,
        json!([{"field":"labelIds","from":[],"to":[{"id": restored_label, "label": "검토 🧪"}]},
               {"field":"assigneeIds","from":[],"to":[{"id": fx.user_id, "label": null}]}])
    );
    assert_eq!(actor, None);

    let root = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_archive_captures_and_publishes_comments_replies_reactions() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "CMT",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let root = project["rootDocumentId"].as_str().unwrap().to_owned();
    let post = |path: String, body: Value| {
        let (app, cookie) = (fx.app.clone(), fx.cookie.clone());
        async move {
            let (status, value) = json_request(app, "POST", &path, Some(body), Some(&cookie)).await;
            assert!(status.is_success(), "{status} {value}");
            value
        }
    };
    let ws = fx.workspace_id;
    // Ordinary flow: a document comment, a reply, a reaction, a resolution and
    // a task comment.
    let top = post(
        format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents/{root}/comments"),
        json!({"body":"  검토 댓글 🧪  "}),
    )
    .await;
    let top_id = top["id"].as_str().unwrap().to_owned();
    let reply = post(
        format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents/{root}/comments"),
        json!({"body":"답글","parentId":top_id}),
    )
    .await;
    post(
        format!("/api/v1/workspaces/{ws}/comments/{top_id}/reactions"),
        json!({"emoji":"👍","on":true}),
    )
    .await;
    post(
        format!("/api/v1/workspaces/{ws}/comments/{top_id}/resolve"),
        json!({}),
    )
    .await;
    let task = post(
        format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        json!({"title":"댓글 태스크 🧪"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_owned();
    let task_comment = post(
        format!("/api/v1/workspaces/{ws}/tasks/{task_id}/comments"),
        json!({"body":"확인"}),
    )
    .await;
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("comments are captured, not refused");
    let rows: Vec<_> = captured
        .archive
        .graph
        .comments
        .iter()
        .map(|c| {
            (
                c.id.to_string(),
                c.document_id.map(|id| id.to_string()),
                c.task_id.map(|id| id.to_string()),
                c.parent_id.map(|id| id.to_string()),
                c.created_by,
                c.body.clone(),
                c.chosung.clone(),
                c.resolved_at.is_some(),
                c.reactions.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                top_id.clone(),
                Some(root.clone()),
                None,
                None,
                fx.user_id,
                "검토 댓글 🧪".to_owned(),
                "ㄱㅌ ㄷㄱ 🧪".to_owned(),
                true,
                json!({"👍":[fx.user_id.to_string()]}),
            ),
            (
                reply["id"].as_str().unwrap().to_owned(),
                Some(root.clone()),
                None,
                Some(top_id.clone()),
                fx.user_id,
                "답글".to_owned(),
                "ㄷㄱ".to_owned(),
                false,
                json!({}),
            ),
            (
                task_comment["id"].as_str().unwrap().to_owned(),
                None,
                Some(task_id.clone()),
                None,
                fx.user_id,
                "확인".to_owned(),
                "ㅎㅇ".to_owned(),
                false,
                json!({}),
            ),
        ]
    );

    // Publish a structural archive carrying a resolved, reacted comment and a
    // reply: same IDs/bodies/parent/timestamps; author and reaction become the
    // destination actor.
    let ids = StructuralIds::fresh();
    let mut archive = structural_archive(&ids);
    let source_actor = archive.graph.source_actor_id;
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    archive.graph.comments = serde_json::from_value(json!([
        {"id": first, "document_id": ids.document, "task_id": null, "parent_id": null,
         "created_by": source_actor, "body": "검토 댓글 🧪", "chosung": "ㄱㅌ ㄷㄱ 🧪",
         "resolved_at": "2026-10-02T00:00:09Z", "reactions": {"👍": [source_actor]},
         "created_at": "2026-10-02T00:00:07Z", "updated_at": "2026-10-02T00:00:09Z"},
        {"id": second, "document_id": ids.document, "task_id": null, "parent_id": first,
         "created_by": source_actor, "body": "답글", "chosung": "ㄷㄱ", "resolved_at": null,
         "reactions": {}, "created_at": "2026-10-02T00:00:08Z", "updated_at": "2026-10-02T00:00:08Z"}
    ]))
    .unwrap();
    archive
        .validate()
        .expect("structural archive with comments is valid");
    let cookie = fx.cookie.clone();
    let (workspace, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    let restored: Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_object('id', id, 'parent', parent_id, 'by', created_by,
            'body', body,
            'resolved', to_char(resolved_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'),
            'reactions', reactions,
            'created', to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
            ORDER BY created_at) FROM fvoci.comments WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(
        restored,
        json!([
            {"id": first, "parent": null, "by": fx.user_id, "body": "검토 댓글 🧪",
             "resolved": "2026-10-02 00:00:09", "reactions": {"👍": [fx.user_id.to_string()]},
             "created": "2026-10-02 00:00:07"},
            {"id": second, "parent": first, "by": fx.user_id, "body": "답글", "resolved": null,
             "reactions": {}, "created": "2026-10-02 00:00:08"}
        ])
    );

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

/// Restricted app role acting as the owner (tenant + self user), the access
/// the 051 RLS grants. Labeled fixture writer: these Zotero rows are NOT the
/// W6 product flow (Tier C routes are not in this tree).
async fn as_owner(
    pool: &sqlx::PgPool,
    workspace: Uuid,
    user: Uuid,
    statements: &[(&str, Vec<Value>)],
) {
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace)
        .await
        .unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, user)
        .await
        .unwrap();
    for (sql, binds) in statements {
        // Values travel as one JSON array so each statement stays typed in SQL.
        sqlx::query(sql)
            .bind(json!(binds))
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    tx.commit().await.unwrap();
}

async fn owner_rows(pool: &sqlx::PgPool, workspace: Uuid, user: Uuid, sql: &str) -> Value {
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace)
        .await
        .unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, user)
        .await
        .unwrap();
    let value: Option<Value> = sqlx::query_scalar(sql)
        .bind(workspace)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    value.unwrap_or(json!([]))
}

#[tokio::test]
async fn native_archive_captures_and_publishes_selected_zotero_closure() {
    use fvoci_server::db::native_archive::{capture, publish, NativeDbError, Selection};
    use fvoci_server::native_archive::{encode, ArchiveError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let (status, personal) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {personal}");
    let ws = Uuid::parse_str(personal["id"].as_str().unwrap()).unwrap();
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "ZOT", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let root = Uuid::parse_str(project["rootDocumentId"].as_str().unwrap()).unwrap();
    let post = |path: String, body: Value| {
        let (app, cookie) = (fx.app.clone(), fx.cookie.clone());
        async move {
            let (status, value) = json_request(app, "POST", &path, Some(body), Some(&cookie)).await;
            assert!(status.is_success(), "{status} {value}");
            Uuid::parse_str(value["id"].as_str().unwrap()).unwrap()
        }
    };
    // Product wiki documents: an ancestor and the backing document under it.
    let ancestor = post(
        format!("/api/v1/workspaces/{ws}/documents"),
        json!({"parentId":null,"title":"참고 문헌 🧪"}),
    )
    .await;
    let backing = post(
        format!("/api/v1/workspaces/{ws}/documents"),
        json!({"parentId":ancestor,"title":"Zotero reference"}),
    )
    .await;
    let task = post(
        format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        json!({"title":"인용 태스크 🧪"}),
    )
    .await;
    let (connector, other_connector, purged) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (doc_link, task_link) = (Uuid::now_v7(), Uuid::now_v7());
    let book = json!({"itemType":"book","title":"합성 연구 자료 🧪","fields":{"date":"Spring 2026"},
        "creators":[{"creatorType":"author","firstName":"민","lastName":"김"}],"tags":[{"tag":"연구","type":0}],"relations":{}});
    let case = json!({"itemType":"case","title":"","fields":{"caseName":"보관 판례"},"creators":[],"tags":[]});
    as_owner(&fx.pool, ws, fx.user_id, &[
        // Operational sync state and a sealed credential exist in the source.
        ("INSERT INTO fvoci.zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url,state,generation,completed_version,progress_version,committed_pages,retry_at,reconciliation_required,sync_id,sync_expires_at,created_at,updated_at)
          SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,'user',42,'https://www.zotero.org/users/42','connected',3,7,9,2,now(),false,(b->>3)::uuid,now()+interval '1 minute','2026-10-01T00:00:01Z','2026-10-01T00:00:02Z' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(connector), json!(ws), json!(fx.user_id), json!(Uuid::now_v7())]),
        ("INSERT INTO fvoci.zotero_credentials(connector_id,workspace_id,owner_user_id,sealed_key) SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,'enc:v2:synthetic-sealed' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(connector), json!(ws), json!(fx.user_id)]),
        ("INSERT INTO fvoci.zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url) SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,'group',77,'https://www.zotero.org/groups/77' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(other_connector), json!(ws), json!(fx.user_id)]),
        ("INSERT INTO fvoci.zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key,availability)
          SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,k,r,n,p,a FROM (SELECT $1::jsonb AS b) q,
          (VALUES ('CHILD234',3,'하위','PARENT23','available'),('PARENT23',2,'상위 🧪',NULL,'available'),('DELETED2',1,'Historical',NULL,'deleted')) AS c(k,r,n,p,a)",
         vec![json!(ws), json!(fx.user_id), json!(connector)]),
        ("INSERT INTO fvoci.zotero_references(id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,local_version,bibliography,return_url,availability)
          SELECT (b->>3)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,'ABCD2345',3,2,b->4,'https://www.zotero.org/users/42/items/ABCD2345','available' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(connector), json!(backing), book.clone()]),
        ("INSERT INTO fvoci.zotero_references(id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,local_version,bibliography,return_url,availability)
          SELECT (b->>3)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,NULL,'EFGH4567',5,1,b->4,'https://www.zotero.org/users/42/items/EFGH4567','deleted' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(connector), json!(purged), case.clone()]),
        ("INSERT INTO fvoci.zotero_memberships(workspace_id,owner_user_id,connector_id,reference_id,collection_key)
          SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,r::uuid,k FROM (SELECT $1::jsonb AS b) q,
          (VALUES (1,'CHILD234'),(2,'DELETED2')) AS m(i,k), LATERAL (SELECT b->>(2+i) AS r) x",
         vec![json!(ws), json!(fx.user_id), json!(connector), json!(backing), json!(purged)]),
        ("INSERT INTO fvoci.zotero_links(id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor)
          SELECT (b->>4)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,(b->>6)::uuid,NULL,'' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(connector), json!(backing), json!(doc_link), json!(task_link), json!(root)]),
        ("INSERT INTO fvoci.zotero_links(id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor)
          SELECT (b->>5)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,NULL,(b->>6)::uuid,'literal-anchor' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(connector), json!(backing), json!(doc_link), json!(task_link), json!(task)]),
    ])
    .await;
    let selected = [connector];
    let captured = capture(
        &fx.pool,
        ws,
        fx.user_id,
        session,
        &Selection {
            project: project_id,
            zotero_connectors: &selected,
        },
    )
    .await
    .expect("selected Zotero closure is captured");
    let g = &captured.archive.graph;
    let wiki: Vec<_> = g
        .documents
        .iter()
        .filter(|d| d.project_id.is_none())
        .map(|d| (d.id, d.parent_id))
        .collect();
    assert_eq!(wiki, vec![(ancestor, None), (backing, Some(ancestor))]);
    assert!(g
        .documents
        .iter()
        .any(|d| d.id == root && d.project_id == Some(project_id)));
    let records = json!({
        "connectors": g.zotero_connectors, "references": g.zotero_references,
        "collections": g.zotero_collections, "memberships": g.zotero_memberships, "links": g.zotero_links
    });
    let connectors = records["connectors"].as_array().unwrap();
    assert_eq!(connectors.len(), 1);
    assert_eq!(connectors[0]["id"], json!(connector));
    assert_eq!(connectors[0]["completed_version"], json!(7));
    assert_eq!(
        connectors[0]["library_url"],
        json!("https://www.zotero.org/users/42")
    );
    assert!(
        chrono::DateTime::parse_from_rfc3339(connectors[0]["created_at"].as_str().unwrap()).is_ok()
    );
    let mut keys: Vec<_> = connectors[0].as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "completed_version",
            "created_at",
            "id",
            "library_type",
            "library_url",
            "remote_library_id",
            "updated_at"
        ]
    );
    assert_eq!(
        records["references"],
        json!([
            {"id":backing,"connector_id":connector,"document_id":backing,"item_key":"ABCD2345","remote_version":3,
             "local_version":2,"bibliography":book,"return_url":"https://www.zotero.org/users/42/items/ABCD2345","availability":"available"},
            {"id":purged,"connector_id":connector,"document_id":null,"item_key":"EFGH4567","remote_version":5,
             "local_version":1,"bibliography":case,"return_url":"https://www.zotero.org/users/42/items/EFGH4567","availability":"deleted"}
        ])
    );
    assert_eq!(
        records["collections"],
        json!([
            {"connector_id":connector,"collection_key":"CHILD234","remote_version":3,"name":"하위","parent_key":"PARENT23","availability":"available"},
            {"connector_id":connector,"collection_key":"DELETED2","remote_version":1,"name":"Historical","parent_key":null,"availability":"deleted"},
            {"connector_id":connector,"collection_key":"PARENT23","remote_version":2,"name":"상위 🧪","parent_key":null,"availability":"available"}
        ])
    );
    let mut memberships = records["memberships"].as_array().unwrap().clone();
    memberships.sort_by_key(|m| m["collection_key"].as_str().unwrap().to_owned());
    assert_eq!(
        memberships,
        vec![
            json!({"connector_id":connector,"reference_id":backing,"collection_key":"CHILD234"}),
            json!({"connector_id":connector,"reference_id":purged,"collection_key":"DELETED2"})
        ]
    );
    let mut links = records["links"].as_array().unwrap().clone();
    links.sort_by_key(|l| l["anchor"].as_str().unwrap().to_owned());
    assert_eq!(
        links,
        vec![
            json!({"id":doc_link,"connector_id":connector,"reference_id":backing,"document_id":root,"task_id":null,"anchor":""}),
            json!({"id":task_link,"connector_id":connector,"reference_id":backing,"document_id":null,"task_id":task,"anchor":"literal-anchor"})
        ]
    );
    let text = serde_json::to_string(&captured.archive).unwrap();
    for secret in [
        "enc:v2",
        "sealed",
        "credential",
        "progress_version",
        "sync_id",
        "retry_at",
        "owner_user_id",
    ] {
        assert!(!text.contains(secret), "{secret}");
    }

    // Selection gates: unknown/unowned connector or a team workspace is a
    // generic denial; a duplicate or oversized selection is invalid.
    let denied = capture(
        &fx.pool,
        ws,
        fx.user_id,
        session,
        &Selection {
            project: project_id,
            zotero_connectors: &[Uuid::now_v7()],
        },
    )
    .await;
    assert!(
        matches!(denied, Err(NativeDbError::Forbidden)),
        "{:?}",
        denied.err()
    );
    let team_project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "TZO",
        "private",
    )
    .await;
    let team_project = Uuid::parse_str(team_project["id"].as_str().unwrap()).unwrap();
    let team = capture(
        &fx.pool,
        fx.workspace_id,
        fx.user_id,
        session,
        &Selection {
            project: team_project,
            zotero_connectors: &selected,
        },
    )
    .await;
    assert!(
        matches!(team, Err(NativeDbError::Forbidden)),
        "{:?}",
        team.err()
    );
    let duplicate = capture(
        &fx.pool,
        ws,
        fx.user_id,
        session,
        &Selection {
            project: project_id,
            zotero_connectors: &[connector, connector],
        },
    )
    .await;
    assert!(
        matches!(
            duplicate,
            Err(NativeDbError::Archive(ArchiveError::Invalid(_)))
        ),
        "{:?}",
        duplicate.err()
    );
    // Route selector: only repeated distinct zoteroConnector UUIDs are accepted.
    for query in [
        "zoteroConnector=not-a-uuid".to_owned(),
        format!("zoteroConnector={connector}&zoteroConnector={connector}"),
        format!("zoteroConnector={connector}&other=1"),
        format!("zoteroconnector={connector}"),
    ] {
        let (status, body) = json_request(
            fx.app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{ws}/projects/{project_id}/native-archive?{query}"),
            None,
            Some(&fx.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query} {body}");
    }
    let (status, body) = json_request(
        fx.app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{ws}/projects/{project_id}/native-archive?zoteroConnector={}",
            Uuid::now_v7()
        ),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Publish: the captured Zotero rows and wiki documents, carried through
    // policy validation on an independent structural project archive.
    let ids = StructuralIds::fresh();
    let mut archive = structural_archive(&ids);
    let source_actor = archive.graph.source_actor_id;
    for d in g.documents.iter().filter(|d| d.project_id.is_none()) {
        let mut d = d.clone();
        d.created_by = source_actor;
        let mut state = archive.graph.states[0].clone();
        state.target_id = d.id;
        state.state_entry = format!("native/document/{}/state.v1", d.id);
        archive
            .entries
            .insert(state.state_entry.clone(), encode(&[0, 0]));
        archive.graph.states.push(state);
        archive.graph.documents.push(d);
    }
    archive.graph.zotero_connectors = g.zotero_connectors.clone();
    archive.graph.zotero_references = g.zotero_references.clone();
    archive.graph.zotero_collections = g.zotero_collections.clone();
    archive.graph.zotero_memberships = g.zotero_memberships.clone();
    archive.graph.zotero_links = g.zotero_links.clone();
    for link in &mut archive.graph.zotero_links {
        // Retarget the source project's document/task to the structural ones.
        if link.document_id.is_some() {
            link.document_id = Some(ids.document);
        } else {
            link.task_id = Some(ids.task);
        }
    }
    archive
        .validate()
        .expect("captured Zotero closure passes the W6 policy");
    // Same database: the preserved wiki/Zotero identities still exist in the
    // source workspace, so publication is a global collision, atomically
    // refused (generic 409 at the route) with no destination rows.
    let collider =
        project_harness::add_workspace_user(&fx.admin, fx.workspace_id, "member", "zcol").await;
    let (collided, _, claim) = claimed_restore(&fx, collider.user_id, &collider.cookie).await;
    let collision = publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await;
    assert!(
        matches!(&collision, Err(NativeDbError::Sql(e)) if e.as_database_error().is_some_and(|e| e.is_unique_violation())),
        "{:?}",
        collision.err()
    );
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1)
            + (SELECT count(*) FROM fvoci.zotero_connectors WHERE workspace_id=$1)
            + (SELECT count(*) FROM fvoci.zotero_references WHERE workspace_id=$1)",
    )
    .bind(collided)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(leftovers, 0);
    // Preservation: a second, isolated installation (own database, roles and
    // pools) receives the archive with every original identity intact.
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("isolated installation receives the original identities");
    let restored = owner_rows(&dst.pool, destination, dst.user_id,
        "SELECT jsonb_agg(jsonb_build_object('id',id,'owner',owner_user_id,'state',state,'generation',generation,
            'completed',completed_version,'progress',progress_version,'pages',committed_pages,'retry',retry_at,
            'reconcile',reconciliation_required,'sync',sync_id,'syncExpires',sync_expires_at,
            'created',to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))) FROM fvoci.zotero_connectors WHERE workspace_id=$1").await;
    assert_eq!(
        restored,
        json!([{"id":connector,"owner":dst.user_id,"state":"disconnected","generation":1,"completed":7,
            "progress":null,"pages":0,"retry":null,"reconcile":true,"sync":null,"syncExpires":null,
            "created":"2026-10-01 00:00:01"}])
    );
    let credentials: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_credentials WHERE workspace_id=$1")
            .bind(destination)
            .fetch_one(&dst.admin)
            .await
            .unwrap();
    assert_eq!(credentials, 0);
    let restored_refs = owner_rows(&dst.pool, destination, dst.user_id,
        "SELECT jsonb_agg(jsonb_build_object('id',id,'document',document_id,'key',item_key,'bibliography',bibliography,'availability',availability) ORDER BY item_key) FROM fvoci.zotero_references WHERE workspace_id=$1").await;
    assert_eq!(
        restored_refs,
        json!([{"id":backing,"document":backing,"key":"ABCD2345","bibliography":book,"availability":"available"},
               {"id":purged,"document":null,"key":"EFGH4567","bibliography":case,"availability":"deleted"}])
    );
    let counts = owner_rows(&dst.pool, destination, dst.user_id,
        "SELECT jsonb_build_array((SELECT count(*) FROM fvoci.zotero_collections WHERE workspace_id=$1),
            (SELECT count(*) FROM fvoci.zotero_memberships WHERE workspace_id=$1),
            (SELECT count(*) FROM fvoci.zotero_links WHERE workspace_id=$1),
            (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1 AND project_id IS NULL))").await;
    assert_eq!(counts, json!([3, 2, 2, 2]));
    let numbers: Vec<(Uuid, Option<Uuid>, i32)> = sqlx::query_as(
        "SELECT id,project_id,number FROM fvoci.documents WHERE workspace_id=$1 AND project_id IS NULL ORDER BY path")
        .bind(destination).fetch_all(&dst.admin).await.unwrap();
    let source_numbers: Vec<(Uuid, Option<Uuid>, i32)> = g
        .documents
        .iter()
        .filter(|d| d.project_id.is_none())
        .map(|d| (d.id, None, d.number))
        .collect();
    assert_eq!(numbers, source_numbers);
    // Owner-private: the source actor sees no rows in the destination.
    let foreign = owner_rows(
        &dst.pool,
        destination,
        fx.user_id,
        "SELECT jsonb_agg(id) FROM fvoci.zotero_connectors WHERE workspace_id=$1",
    )
    .await;
    assert_eq!(foreign, json!([]));

    // Refusals before any effect: an unselected connector's link into the
    // selection, and an omitted wiki descendant of the closure.
    let stray = Uuid::now_v7();
    as_owner(&fx.pool, ws, fx.user_id, &[
        ("INSERT INTO fvoci.zotero_references(id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,bibliography,return_url,availability)
          SELECT (b->>3)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,NULL,'JKLM2345',1,b->4,'https://www.zotero.org/groups/77/items/JKLM2345','available' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(other_connector), json!(stray), case.clone()]),
        ("INSERT INTO fvoci.zotero_links(id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor)
          SELECT (b->>4)::uuid,(b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,(b->>5)::uuid,NULL,'' FROM (SELECT $1::jsonb AS b) q",
         vec![json!(ws), json!(fx.user_id), json!(other_connector), json!(stray), json!(Uuid::now_v7()), json!(root)]),
    ])
    .await;
    for selection in [&selected[..], &[][..]] {
        let refused = capture(
            &fx.pool,
            ws,
            fx.user_id,
            session,
            &Selection {
                project: project_id,
                zotero_connectors: selection,
            },
        )
        .await;
        assert!(
            matches!(&refused, Err(NativeDbError::Archive(ArchiveError::Unsupported(m))) if m == "zotero links"),
            "{:?}",
            refused.err()
        );
    }
    as_owner(
        &fx.pool,
        ws,
        fx.user_id,
        &[(
            "DELETE FROM fvoci.zotero_links WHERE connector_id=($1::jsonb->>0)::uuid",
            vec![json!(other_connector)],
        )],
    )
    .await;
    capture(
        &fx.pool,
        ws,
        fx.user_id,
        session,
        &Selection {
            project: project_id,
            zotero_connectors: &selected,
        },
    )
    .await
    .expect("closure is capturable again");
    post(
        format!("/api/v1/workspaces/{ws}/documents"),
        json!({"parentId":backing,"title":"하위 메모"}),
    )
    .await;
    let omitted = capture(
        &fx.pool,
        ws,
        fx.user_id,
        session,
        &Selection {
            project: project_id,
            zotero_connectors: &selected,
        },
    )
    .await;
    assert!(
        matches!(&omitted, Err(NativeDbError::Archive(ArchiveError::Unsupported(m))) if m == "wiki closure"),
        "{:?}",
        omitted.err()
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_restores_personal_input_origin_and_retired_receipt() {
    use fvoci_server::db::native_archive::{capture, publish, NativeDbError};
    use fvoci_server::native_archive::{encode, ArchiveError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let (status, personal) = json_request(
        fx.app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {personal}");
    let ws = Uuid::parse_str(personal["id"].as_str().unwrap()).unwrap();
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "PIN", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    // Real personal-input capture: origin wiki document, task, origin row and
    // the capture receipt, all written by the product route.
    let request = Uuid::now_v7();
    let body = json!({"requestId":request,"intent":"task","title":"개인 입력 태스크 🧪","projectId":project_id});
    let (status, created) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/personal-input"),
        Some(body.clone()),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let origin = Uuid::parse_str(created["documentId"].as_str().unwrap()).unwrap();
    let task = Uuid::parse_str(created["taskId"].as_str().unwrap()).unwrap();
    assert_eq!(created["projectId"], json!(project_id));
    let (source_hash, origin_hash): (String, String) = sqlx::query_as(
        "SELECT c.request_hash,o.request_hash FROM fvoci.personal_input_commands c
         JOIN fvoci.task_origins o ON o.workspace_id=c.workspace_id AND o.request_id=c.request_id
         WHERE c.workspace_id=$1 AND c.request_id=$2",
    )
    .bind(ws)
    .bind(request)
    .fetch_one(&fx.admin)
    .await
    .unwrap();

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("personal-input origin and receipt are captured, not refused");
    let g = &captured.archive.graph;
    let wiki: Vec<_> = g
        .documents
        .iter()
        .filter(|d| d.project_id.is_none())
        .map(|d| d.id)
        .collect();
    assert_eq!(wiki, vec![origin]);
    assert_eq!(
        g.origins
            .iter()
            .map(|o| (
                o.task_id,
                o.document_id,
                o.request_id,
                o.request_hash.clone()
            ))
            .collect::<Vec<_>>(),
        vec![(task, origin, request, origin_hash.clone())]
    );
    let receipts = serde_json::to_value(&g.personal_input_commands).unwrap();
    assert_eq!(receipts.as_array().unwrap().len(), 1);
    assert_eq!(receipts[0]["request_id"], json!(request));
    assert_eq!(receipts[0]["request_hash"], json!(source_hash));
    assert_eq!(receipts[0]["intent"], json!("task"));
    assert_eq!(
        (
            &receipts[0]["document_id"],
            &receipts[0]["task_id"],
            &receipts[0]["project_id"]
        ),
        (&json!(origin), &json!(task), &json!(project_id))
    );

    // Publish into a second isolated installation through policy validation:
    // a structural project carrying the original project/task identities plus
    // the captured origin document, origin row and receipt unchanged.
    let mut ids = StructuralIds::fresh();
    ids.project = project_id;
    ids.task = task;
    let mut archive = structural_archive(&ids);
    let source_actor = archive.graph.source_actor_id;
    let mut d = g.documents.iter().find(|d| d.id == origin).unwrap().clone();
    d.created_by = source_actor;
    let mut state = archive.graph.states[0].clone();
    state.target_id = origin;
    state.state_entry = format!("native/document/{origin}/state.v1");
    archive
        .entries
        .insert(state.state_entry.clone(), encode(&[0, 0]));
    archive.graph.states.push(state);
    archive.graph.documents.push(d);
    archive.graph.origins = g.origins.clone();
    archive.graph.personal_input_commands = g.personal_input_commands.clone();
    archive
        .validate()
        .expect("origin wiki and retired receipt pass the policy");
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("isolated installation receives the origin closure");
    let restored: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('actor',actor_user_id,'hash',request_hash,'intent',intent,
            'document',document_id,'task',task_id,'project',project_id,
            'sameCreatedAt',created_at=$3::timestamptz)
         FROM fvoci.personal_input_commands WHERE workspace_id=$1 AND request_id=$2",
    )
    .bind(destination)
    .bind(request)
    .bind(receipts[0]["created_at"].as_str().unwrap())
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(
        restored,
        json!({"actor":dst.user_id,"hash":source_hash,"intent":"task","document":null,
            "task":null,"project":null,"sameCreatedAt":true})
    );
    let origin_row: (Uuid, Uuid, String, Option<Uuid>) = sqlx::query_as(
        "SELECT o.task_id,o.request_id,o.request_hash,d.project_id FROM fvoci.task_origins o
         JOIN fvoci.documents d ON d.workspace_id=o.workspace_id AND d.id=o.document_id
         WHERE o.workspace_id=$1 AND o.document_id=$2",
    )
    .bind(destination)
    .bind(origin)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(origin_row, (task, request, origin_hash, None));
    // The imported receipt is never a live replay target: the same command in
    // the destination is a hash mismatch (source workspace/actor bound), 409,
    // with no new document, task or receipt.
    let counts = |admin: sqlx::PgPool| async move {
        sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1),
                    (SELECT count(*) FROM fvoci.tasks WHERE workspace_id=$1),
                    (SELECT count(*) FROM fvoci.personal_input_commands WHERE workspace_id=$1)",
        )
        .bind(destination)
        .fetch_one(&admin)
        .await
        .unwrap()
    };
    let before = counts(dst.admin.clone()).await;
    let (status, replay) = json_request(
        dst.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{destination}/personal-input"),
        Some(body),
        Some(&dst.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{replay}");
    assert_eq!(counts(dst.admin.clone()).await, before);

    // An omitted wiki child of the origin document is refused, not pruned.
    let (status, child) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"parentId":origin,"title":"하위 메모"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {child}");
    let omitted = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id)).await;
    assert!(
        matches!(&omitted, Err(NativeDbError::Archive(ArchiveError::Unsupported(m))) if m == "wiki closure"),
        "{:?}",
        omitted.err()
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_carries_exact_numeric_task_estimate() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let project = project_harness::create_project(
        fx.app.clone(),
        &fx.cookie,
        fx.workspace_id,
        "EST",
        "private",
    )
    .await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let ws = fx.workspace_id;
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"추정 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = task["id"].as_str().unwrap().to_owned();
    // More significant digits than an f64 keeps: a JSON number would round.
    let exact = "123456789012.123456";
    let (status, patched) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}"),
        Some(json!({"estimate":exact})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("a task estimate is captured, not refused");
    assert_eq!(
        captured
            .archive
            .graph
            .tasks
            .iter()
            .map(|t| t.estimate.clone())
            .collect::<Vec<_>>(),
        vec![Some(json!(exact))]
    );
    assert_eq!(captured.archive.graph.tasks[0].estimate_unit, None);

    let ids = StructuralIds::fresh();
    let mut archive = structural_archive(&ids);
    archive.graph.tasks[0].estimate = Some(json!(exact));
    archive
        .validate()
        .expect("exact decimal text is a valid estimate");
    let cookie = fx.cookie.clone();
    let (workspace, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    let restored: String = sqlx::query_scalar(
        "SELECT estimate::text FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(ids.task)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(restored, exact);

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_current_estimate_unit_restore_provenance_and_timer_run() {
    use fvoci_server::collab::wire::CollabKind;
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "CUR", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let root = Uuid::parse_str(project["rootDocumentId"].as_str().unwrap()).unwrap();
    let (status, document) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents"),
        Some(json!({"title":"복원 이력 🧪", "parentId":root})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{document}");
    let document = Uuid::parse_str(document["id"].as_str().unwrap()).unwrap();
    let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"복원 대상"}]}]});
    let (_, manual) = product_native_write(
        &fx,
        session,
        project_id,
        CollabKind::Document,
        document,
        body,
    )
    .await;
    let tail: i64 =
        sqlx::query_scalar("SELECT tail_seq FROM fvoci.document_states WHERE document_id=$1")
            .bind(document)
            .fetch_one(&fx.admin)
            .await
            .unwrap();
    assert!(tail >= 1);
    // A current 050 'restore' row (fixture writer through the restricted app
    // role: the W4 restore writer is not in this tree) over that history.
    let (restored, correlation) = (Uuid::now_v7(), Uuid::now_v7());
    let in_tenant = |sql: &'static str, binds: Vec<Value>| {
        let pool = fx.pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            fvoci_server::db::context::set_tenant(&mut tx, ws)
                .await
                .unwrap();
            fvoci_server::db::context::set_self_user(&mut tx, fx.user_id)
                .await
                .unwrap();
            let result = sqlx::query(sql).bind(json!(binds)).execute(&mut *tx).await;
            if result.is_ok() {
                tx.commit().await.unwrap();
            }
            result.map(|_| ())
        }
    };
    let insert_restore = "INSERT INTO fvoci.revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_by,restored_from_id,restore_correlation_id,restore_base_tail_seq,restore_committed_tail_seq)
        SELECT (b->>0)::uuid,r.workspace_id,r.target_kind,r.target_id,r.y_snapshot,r.encoding,r.content_json,r.text,'restore',r.created_by,r.id,(b->>2)::uuid,(b->>3)::bigint,(b->>4)::bigint
        FROM fvoci.revisions r, (SELECT $1::jsonb AS b) q WHERE r.id=(b->>1)::uuid";
    // The schema itself refuses non-adjacent tails.
    let bad = in_tenant(
        insert_restore,
        vec![
            json!(Uuid::now_v7()),
            json!(manual),
            json!(Uuid::now_v7()),
            json!(tail - 1),
            json!(tail + 1),
        ],
    )
    .await;
    assert!(
        bad.as_ref()
            .err()
            .and_then(|e| e.as_database_error())
            .and_then(|e| e.code())
            .as_deref()
            == Some("23514"),
        "{bad:?}"
    );
    in_tenant(
        insert_restore,
        vec![
            json!(restored),
            json!(manual),
            json!(correlation),
            json!(tail - 1),
            json!(tail),
        ],
    )
    .await
    .unwrap();
    // Retained history: the content digest covers the restore provenance.
    let cancel = CancellationToken::new();
    let first = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert!(first.revisions.contains(&restored) && first.revisions.contains(&manual));
    in_tenant(
        "UPDATE fvoci.revisions SET restore_correlation_id=($1::jsonb->>1)::uuid WHERE id=($1::jsonb->>0)::uuid",
        vec![json!(restored), json!(Uuid::now_v7())],
    )
    .await
    .unwrap();
    let moved = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(moved.content_digest, first.content_digest);
    in_tenant(
        "UPDATE fvoci.revisions SET restore_correlation_id=($1::jsonb->>1)::uuid WHERE id=($1::jsonb->>0)::uuid",
        vec![json!(restored), json!(correlation)],
    )
    .await
    .unwrap();
    let back = locked_inventory(&fx, CollabKind::Document, document, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.content_digest, first.content_digest);

    // A current 049 minutes estimate (fixture writer; the W5 minutes command
    // is not in this tree) and the captured graph's exact current fields.
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"분 단위 추정 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let (status, patched) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}"),
        Some(json!({"estimate":"90"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    in_tenant(
        "UPDATE fvoci.tasks SET estimate_unit='minutes' WHERE id=($1::jsonb->>0)::uuid",
        vec![json!(task_id)],
    )
    .await
    .unwrap();
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("current estimate unit and restore provenance decode");
    let g = &captured.archive.graph;
    let t = g.tasks.iter().find(|t| t.id == task_id).unwrap();
    assert_eq!(
        (t.estimate.clone(), t.estimate_unit.clone()),
        (Some(json!("90")), Some("minutes".to_owned()))
    );
    let r = g.revisions.iter().find(|r| r.id == restored).unwrap();
    assert_eq!(
        (
            r.reason.as_str(),
            r.restored_from_id,
            r.restore_correlation_id,
            r.restore_base_tail_seq,
            r.restore_committed_tail_seq
        ),
        (
            "restore",
            Some(manual),
            Some(correlation),
            Some(tail - 1),
            Some(tail)
        )
    );
    let m = g.revisions.iter().find(|r| r.id == manual).unwrap();
    assert_eq!(
        (
            m.restored_from_id,
            m.restore_correlation_id,
            m.restore_base_tail_seq,
            m.restore_committed_tail_seq
        ),
        (None, None, None, None)
    );

    // Publish the current fields through policy validation (structural graph
    // with an archived tail) into a fresh personal workspace.
    let ids = StructuralIds::fresh();
    let mut archive = structural_archive(&ids);
    archive.graph.tasks[0].estimate = Some(json!("90"));
    archive.graph.tasks[0].estimate_unit = Some("minutes".into());
    let doc_state = archive
        .graph
        .states
        .iter_mut()
        .find(|s| s.target_kind == "document")
        .unwrap();
    doc_state.snapshot_cutoff_seq = 5;
    doc_state.tail_seq = 5;
    let (source_revision, archived_restore, archived_correlation) =
        (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let source_actor = archive.graph.source_actor_id;
    let content = archive.graph.documents[0].content_json.clone();
    let text = archive.graph.documents[0].text.clone();
    for (id, reason) in [(source_revision, "manual"), (archived_restore, "restore")] {
        let mut revision: fvoci_server::native_archive::Revision = serde_json::from_value(json!({
            "id":id,"target_kind":"document","target_id":ids.document,
            "snapshot_entry":format!("revisions/{id}.snapshot.v1"),"encoding":1,"content_json":content,
            "text":text,"reason":reason,"created_by":source_actor,"created_at":"2026-10-02T00:00:05Z"}))
        .unwrap();
        if reason == "restore" {
            revision.restored_from_id = Some(source_revision);
            revision.restore_correlation_id = Some(archived_correlation);
            revision.restore_base_tail_seq = Some(4);
            revision.restore_committed_tail_seq = Some(5);
        }
        archive.entries.insert(
            revision.snapshot_entry.clone(),
            fvoci_server::native_archive::encode(&[0, 0]),
        );
        archive.graph.revisions.push(revision);
    }
    archive
        .validate()
        .expect("current fields pass the archive policy");
    let cookie = fx.cookie.clone();
    let (workspace, _, claim) = claimed_restore(&fx, fx.user_id, &cookie).await;
    publish(
        &fx.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &fx.settings.quota,
    )
    .await
    .unwrap();
    let restored_task: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT estimate::text,estimate_unit FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(ids.task)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(restored_task, (Some("90".into()), Some("minutes".into())));
    let restored_revision: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('reason',reason,'by',created_by,'from',restored_from_id,'correlation',restore_correlation_id,'base',restore_base_tail_seq,'committed',restore_committed_tail_seq)
         FROM fvoci.revisions WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace)
    .bind(archived_restore)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(
        restored_revision,
        json!({"reason":"restore","by":fx.user_id,"from":source_revision,"correlation":archived_correlation,"base":4,"committed":5})
    );

    // A current 048 stopwatch on a selected task is now carried (it was the
    // typed "task timers" refusal before timer support), never omitted.
    let run = Uuid::now_v7();
    in_tenant(
        "INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at,stopped_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,'stopped',1,now()-interval '1 hour',now() FROM (SELECT $1::jsonb AS b) q",
        vec![json!(run), json!(fx.user_id), json!(ws), json!(task_id)],
    )
    .await
    .unwrap();
    let carried = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("a stopped stopwatch run is captured");
    assert_eq!(
        carried
            .archive
            .graph
            .timer_runs
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![run]
    );

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_carries_bodies_without_native_state() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "NST", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"열지 않은 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    // Ordinary API objects never opened in an editor have no native state.
    let states: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM fvoci.task_states WHERE task_id=$1)
              + (SELECT count(*) FROM fvoci.document_states s JOIN fvoci.documents d ON d.id=s.document_id WHERE d.project_id=$2)",
    )
    .bind(task_id)
    .bind(project_id)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert_eq!(states, 0);
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("body-only targets are captured");
    let archive = captured.archive;
    assert!(archive.graph.states.is_empty());
    assert!(!archive.graph.documents.is_empty() && !archive.graph.tasks.is_empty());
    // Every captured stateless body is the empty document the product can
    // open later (ensure_collab_state_row accepts only that body).
    let empty = fvoci_server::db::documents::empty_document_json();
    assert!(archive.graph.tasks.iter().all(|t| t.content_json == empty));
    assert!(archive
        .graph
        .documents
        .iter()
        .all(|d| d.content_json == empty));
    archive
        .validate()
        .expect("empty body-only targets are a valid current archive");
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    // A stateless NON-empty body could never be opened at the destination:
    // refused before publication, with no destination effect at all.
    let mut nonempty = archive.clone();
    let task = nonempty
        .graph
        .tasks
        .iter_mut()
        .find(|t| t.id == task_id)
        .unwrap();
    task.content_json = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"본문"}]}]});
    task.text = "본문".into();
    task.chosung = "ㅂㅁ".into();
    assert!(matches!(
        nonempty.validate(),
        Err(fvoci_server::native_archive::ArchiveError::Unsupported(m)) if m == "missing native state for captured body"
    ));
    let probe =
        project_harness::add_workspace_user(&dst.admin, dst.workspace_id, "member", "nst").await;
    let (probe_workspace, _, probe_claim) =
        claimed_restore(&dst, probe.user_id, &probe.cookie).await;
    let refused = publish(
        &dst.pool,
        &probe_claim,
        &nonempty,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await;
    assert!(refused.is_err(), "{refused:?}");
    let effects: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1)
              + (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1)
              + (SELECT count(*) FROM fvoci.tasks WHERE workspace_id=$1)
              + (SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND verb LIKE 'native_archive%')",
    )
    .bind(probe_workspace)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(effects, 0);
    // A separate installation receives the same empty bodies, still stateless.
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("body-only targets restore");
    let restored: (Value, String, i64) = sqlx::query_as(
        "SELECT t.content_json,t.title,(SELECT count(*) FROM fvoci.task_states WHERE task_id=t.id)
         FROM fvoci.tasks t WHERE t.workspace_id=$1 AND t.id=$2",
    )
    .bind(destination)
    .bind(task_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    let source = archive
        .graph
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .unwrap();
    assert_eq!(
        restored,
        (source.content_json.clone(), source.title.clone(), 0)
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

/// Restricted app role in tenant + self-actor context (the 048 tables are
/// actor-self); a labeled fixture writer for rows whose W5 writers are not in
/// this tree. One JSON array parameter keeps each statement typed in SQL.
async fn as_actor(pool: &sqlx::PgPool, workspace: Uuid, actor: Uuid, sql: &str, binds: Vec<Value>) {
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace)
        .await
        .unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, actor)
        .await
        .unwrap();
    sqlx::query(sql)
        .bind(json!(binds))
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn native_archive_restores_task_time_with_retired_receipts() {
    use fvoci_server::db::native_archive::{capture, publish, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "TIM", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"시간 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    // A closed entry through the ordinary route.
    let (status, entry) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}/time-entries"),
        Some(json!({"startedAt":"2026-10-02T09:00:00.000Z","endedAt":"2026-10-02T10:00:00.000Z","note":"기록 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {entry}");
    let closed = Uuid::parse_str(entry["id"].as_str().unwrap()).unwrap();
    // 048 rows written by the restricted role as the actor (W5 writers are
    // not in this tree): a stopped run with its projected segment, a receipt
    // and a correction audit; then an open entry whose 034 trigger creates
    // the legacy reservation.
    let (run, segment, request, audit, open) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let b = |values: &[Uuid]| values.iter().map(|v| json!(v)).collect::<Vec<_>>();
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at,stopped_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,'stopped',3,'2026-10-02T09:00:00Z','2026-10-02T10:00:00Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[run, fx.user_id, ws, task_id])).await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_segments(id,run_id,user_id,workspace_id,task_id,started_at,ended_at,time_entry_id)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,(b->>4)::uuid,'2026-10-02T09:00:00Z','2026-10-02T10:00:00Z',(b->>5)::uuid FROM (SELECT $1::jsonb AS b) q",
        b(&[segment, run, fx.user_id, ws, task_id, closed])).await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_commands(user_id,request_id,request_hash,run_id,result,created_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,repeat('c',64),(b->>2)::uuid,jsonb_build_object('runId',b->>2,'status','stopped'),'2026-10-02T10:00:01Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[fx.user_id, request, run])).await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,(b->>4)::uuid,(b->>5)::uuid,'time.correct',jsonb_build_object('recordId',b->>5,'kind','manual'),jsonb_build_object('recordId',b->>5,'kind','manual','revision',2),'정정','2026-10-02T11:00:00Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[audit, fx.user_id, request, ws, task_id, closed])).await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at) SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,'2026-10-02T12:00:00Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[open, ws, task_id, fx.user_id])).await;
    // The W5 legacy release shape (src/db/task_timer.rs at 78c8): the actor's
    // reservation is removed, an audit locates only the entry (workspace and
    // task NULL) and its receipt has no run. The open 034 row stays.
    let (release, release_audit) = (Uuid::now_v7(), Uuid::now_v7());
    as_actor(
        &fx.pool,
        ws,
        fx.user_id,
        "DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id=($1::jsonb->>0)::uuid",
        b(&[open]),
    )
    .await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,NULL,NULL,(b->>3)::uuid,'legacy.release',jsonb_build_object('timeEntryId',b->>3),jsonb_build_object('released',true),'explicit_release_original_range_unresolved','2026-10-02T12:30:00Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[release_audit, fx.user_id, release, open])).await;
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_commands(user_id,request_id,request_hash,run_id,result,created_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,repeat('d',64),NULL,jsonb_build_object('timeEntryId',b->>2,'released',true),'2026-10-02T12:30:01Z' FROM (SELECT $1::jsonb AS b) q",
        b(&[fx.user_id, release, open])).await;

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("task time is captured");
    let g = &captured.archive.graph;
    let mut entries: Vec<_> = g
        .time_entries
        .iter()
        .map(|e| (e.id, e.duration_seconds))
        .collect();
    entries.sort();
    let mut expected = vec![(closed, Some(3600)), (open, None)];
    expected.sort();
    assert_eq!(entries, expected);
    assert_eq!(
        g.timer_runs.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![run]
    );
    assert_eq!(
        g.timer_segments
            .iter()
            .map(|s| (s.id, s.time_entry_id))
            .collect::<Vec<_>>(),
        vec![(segment, Some(closed))]
    );
    // The released reservation is absent; the release receipt travels with
    // its entry-only audit.
    assert!(g.timer_legacy_open.is_empty());
    assert_eq!(
        g.timer_commands
            .iter()
            .map(|c| (c.request_id, c.run_id))
            .collect::<Vec<_>>(),
        vec![(request, Some(run)), (release, None)]
    );
    assert_eq!(
        g.timer_audit
            .iter()
            .map(|a| (a.id, a.workspace_id, a.task_id, a.time_entry_id))
            .collect::<Vec<_>>(),
        vec![
            (audit, Some(ws), Some(task_id), Some(closed)),
            (release_audit, None, None, Some(open))
        ]
    );
    let archive = captured.archive;
    archive
        .validate()
        .expect("task time passes the archive policy");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    // A destination person who already has an unfinished stopwatch cannot
    // receive an open entry: refused before any effect.
    let busy =
        project_harness::add_workspace_user(&dst.admin, dst.workspace_id, "owner", "busy").await;
    let busy_project = project_harness::create_project(
        dst.app.clone(),
        &busy.cookie,
        dst.workspace_id,
        "BSY",
        "private",
    )
    .await;
    let busy_project_id = Uuid::parse_str(busy_project["id"].as_str().unwrap()).unwrap();
    let (status, busy_task) = json_request(
        dst.app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/projects/{busy_project_id}/tasks",
            dst.workspace_id
        ),
        Some(json!({"title":"진행 중"})),
        Some(&busy.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {busy_task}");
    let busy_task = Uuid::parse_str(busy_task["id"].as_str().unwrap()).unwrap();
    as_actor(&dst.pool, dst.workspace_id, busy.user_id,
        "INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,(b->>2)::uuid,(b->>3)::uuid,'running',1,now() FROM (SELECT $1::jsonb AS b) q",
        b(&[Uuid::now_v7(), busy.user_id, dst.workspace_id, busy_task])).await;
    let (busy_workspace, _, busy_claim) = claimed_restore(&dst, busy.user_id, &busy.cookie).await;
    let refused = publish(
        &dst.pool,
        &busy_claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await;
    assert!(
        matches!(refused, Err(NativeDbError::Conflict)),
        "{refused:?}"
    );
    let effects: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1)+(SELECT count(*) FROM fvoci.time_entries WHERE workspace_id=$1)",
    )
    .bind(busy_workspace)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(effects, 0);

    // The owner receives every row with its identity, as the destination actor.
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("task time restores");
    let restored: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'entries',(SELECT jsonb_agg(jsonb_build_object('id',id,'user',user_id,'seconds',duration_seconds,'note',note) ORDER BY started_at) FROM fvoci.time_entries WHERE workspace_id=$1),
            'reservations',(SELECT coalesce(jsonb_agg(time_entry_id),'[]'::jsonb) FROM fvoci.task_timer_legacy_open WHERE workspace_id=$1),
            'runs',(SELECT jsonb_agg(jsonb_build_object('id',id,'user',user_id,'status',status,'version',version)) FROM fvoci.task_timer_runs WHERE workspace_id=$1),
            'segments',(SELECT jsonb_agg(jsonb_build_object('id',id,'run',run_id,'entry',time_entry_id)) FROM fvoci.task_timer_segments WHERE workspace_id=$1),
            'commands',(SELECT jsonb_agg(jsonb_build_object('request',request_id,'user',user_id,'hash',request_hash,'run',run_id,'result',result,'restoredFrom',restored_from_archive) ORDER BY created_at) FROM fvoci.task_timer_commands WHERE user_id=$2),
            'audit',(SELECT jsonb_agg(jsonb_build_object('id',id,'user',user_id,'workspace',workspace_id,'task',task_id,'entry',time_entry_id,'reason',reason) ORDER BY created_at) FROM fvoci.task_timer_audit WHERE user_id=$2))",
    )
    .bind(destination)
    .bind(dst.user_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(
        restored,
        json!({
            "entries":[{"id":closed,"user":dst.user_id,"seconds":3600,"note":"기록 🧪"},{"id":open,"user":dst.user_id,"seconds":null,"note":null}],
            "reservations":[],
            "runs":[{"id":run,"user":dst.user_id,"status":"stopped","version":3}],
            "segments":[{"id":segment,"run":run,"entry":closed}],
            "commands":[{"request":request,"user":dst.user_id,"hash":"c".repeat(64),"run":run,
                "result":{"runId":run.to_string(),"status":"stopped"},"restoredFrom":claim.job_id},
                {"request":release,"user":dst.user_id,"hash":"d".repeat(64),"run":null,
                "result":{"timeEntryId":open.to_string(),"released":true},"restoredFrom":claim.job_id}],
            "audit":[{"id":audit,"user":dst.user_id,"workspace":destination,"task":task_id,"entry":closed,"reason":"정정"},
                {"id":release_audit,"user":dst.user_id,"workspace":null,"task":null,"entry":open,
                "reason":"explicit_release_original_range_unresolved"}]
        })
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

/// Current actor and session as the W5 command guards expect them.
async fn timer_body(fx: &Fixture, cookie: &str, mut body: Value) -> Value {
    let (status, me) =
        json_request(fx.app.clone(), "GET", "/api/v1/auth/me", None, Some(cookie)).await;
    assert!(status.is_success(), "{status} {me}");
    body["expectedActorId"] = me["userId"].clone();
    body["expectedSessionId"] = me["sessionId"].clone();
    body
}

async fn timer_post(
    fx: &Fixture,
    cookie: &str,
    path: &str,
    body: &Value,
) -> (axum::http::StatusCode, Value) {
    json_request(
        fx.app.clone(),
        "POST",
        path,
        Some(body.clone()),
        Some(cookie),
    )
    .await
}

/// Every timer/time row of one person (admin read, test oracle only).
async fn timer_graph(admin: &sqlx::PgPool, user: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'entries',(SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY e.id),'[]') FROM fvoci.time_entries e WHERE e.user_id=$1),
            'runs',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]') FROM fvoci.task_timer_runs r WHERE r.user_id=$1),
            'segments',(SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY s.id),'[]') FROM fvoci.task_timer_segments s WHERE s.user_id=$1),
            'reservations',(SELECT coalesce(jsonb_agg(to_jsonb(l) ORDER BY l.time_entry_id),'[]') FROM fvoci.task_timer_legacy_open l WHERE l.user_id=$1),
            'commands',(SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.request_id),'[]') FROM fvoci.task_timer_commands c WHERE c.user_id=$1),
            'audit',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.id),'[]') FROM fvoci.task_timer_audit a WHERE a.user_id=$1),
            'estimates',(SELECT coalesce(jsonb_agg(jsonb_build_array(t.id,t.estimate::text,t.estimate_unit) ORDER BY t.id),'[]') FROM fvoci.tasks t WHERE t.workspace_id IN(SELECT workspace_id FROM fvoci.memberships WHERE user_id=$1)))",
    )
    .bind(user)
    .fetch_one(admin)
    .await
    .unwrap()
}

/// The same person (same user id) on a second installation, as an operator
/// restore of an account would have it: user, membership and a session.
async fn same_person(fx: &Fixture, user: Uuid) -> String {
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Same person')")
        .bind(user)
        .bind(format!("same-{user}@example.com"))
        .execute(&fx.admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(fx.workspace_id)
    .bind(user)
    .execute(&fx.admin)
    .await
    .unwrap();
    let token = fvoci_server::auth::token::new_token();
    sqlx::query("INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + interval '1 hour')")
        .bind(Uuid::now_v7())
        .bind(user)
        .bind(&token.hash)
        .execute(&fx.admin)
        .await
        .unwrap();
    token.token
}

#[tokio::test]
async fn native_archive_restored_timer_receipts_never_replay_and_new_commands_work() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "RPL", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let mut tasks = Vec::new();
    for title in ["견적과 기록 🧪", "정리 대상", "레거시 열린 기록"] {
        let (status, task) = json_request(
            fx.app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
            Some(json!({"title":title})),
            Some(&fx.cookie),
        )
        .await;
        assert!(status.is_success(), "{status} {task}");
        tasks.push(Uuid::parse_str(task["id"].as_str().unwrap()).unwrap());
    }
    let base =
        |workspace: Uuid, task: Uuid| format!("/api/v1/workspaces/{workspace}/tasks/{task}/timer");
    // Every W5 command kind through the real routes, as the source person.
    let mut old: Vec<(&str, String, Value)> = Vec::new();
    let (status, state) = json_request(
        fx.app.clone(),
        "GET",
        &base(ws, tasks[0]),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {state}");
    let estimate = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"expected":state["estimate"],"minutes":90,"reason":"계획 견적"})).await;
    let (status, source_estimate) = timer_post(
        &fx,
        &fx.cookie,
        &format!("{}/estimate", base(ws, tasks[0])),
        &estimate,
    )
    .await;
    assert!(status.is_success(), "{status} {source_estimate}");
    old.push((
        "estimate",
        format!(
            "{}/estimate",
            base(ws, tasks[0]).replace(&ws.to_string(), "{ws}")
        ),
        estimate,
    ));
    let start = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"읽기"})).await;
    let (status, started) = timer_post(&fx, &fx.cookie, &base(ws, tasks[0]), &start).await;
    assert!(status.is_success(), "{status} {started}");
    let stop = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"stop","expectedVersion":started["version"],"runId":started["runId"]})).await;
    let (status, stopped) = timer_post(&fx, &fx.cookie, &base(ws, tasks[0]), &stop).await;
    assert!(status.is_success(), "{status} {stopped}");
    old.push((
        "start",
        base(ws, tasks[0]).replace(&ws.to_string(), "{ws}"),
        start,
    ));
    old.push((
        "stop",
        base(ws, tasks[0]).replace(&ws.to_string(), "{ws}"),
        stop,
    ));
    let manual = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-10-02T09:00:00Z","endedAt":"2026-10-02T10:00:00Z","note":"수동","reason":"수동 기록"})).await;
    let (status, created) = timer_post(
        &fx,
        &fx.cookie,
        &format!("{}/history", base(ws, tasks[0])),
        &manual,
    )
    .await;
    assert!(status.is_success(), "{status} {created}");
    old.push((
        "manual",
        format!(
            "{}/history",
            base(ws, tasks[0]).replace(&ws.to_string(), "{ws}")
        ),
        manual,
    ));
    let record = created["record"]["id"].as_str().unwrap().to_string();
    let correct = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"kind":"manual","expectedRevision":0,"expectedStartedAt":"2026-10-02T09:00:00Z","expectedEndedAt":"2026-10-02T10:00:00Z","expectedNote":"수동","startedAt":"2026-10-02T09:00:00Z","endedAt":"2026-10-02T09:30:00Z","note":"정정","reason":"휴식 제외"})).await;
    let correct_path = format!("{}/records/{record}/correct", base(ws, tasks[0]));
    let (status, corrected) = timer_post(&fx, &fx.cookie, &correct_path, &correct).await;
    assert!(status.is_success(), "{status} {corrected}");
    old.push((
        "correct",
        correct_path.replace(&ws.to_string(), "{ws}"),
        correct,
    ));
    let start2 = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":null})).await;
    let (status, started2) = timer_post(&fx, &fx.cookie, &base(ws, tasks[1]), &start2).await;
    assert!(status.is_success(), "{status} {started2}");
    old.push((
        "start (later cleaned up)",
        base(ws, tasks[1]).replace(&ws.to_string(), "{ws}"),
        start2.clone(),
    ));
    let cleanup = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"runId":started2["runId"],"expectedVersion":started2["version"]})).await;
    let (status, cleaned) =
        timer_post(&fx, &fx.cookie, "/api/v1/me/task-timer/stop", &cleanup).await;
    assert!(status.is_success(), "{status} {cleaned}");
    old.push(("cleanup", "/api/v1/me/task-timer/stop".into(), cleanup));
    let (status, open) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{}/time-entries", tasks[2]),
        Some(json!({"startedAt":"2026-10-02T12:00:00Z","note":"열린 기록"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {open}");
    let release = timer_body(
        &fx,
        &fx.cookie,
        json!({"requestId":Uuid::now_v7(),"timeEntryId":open["id"]}),
    )
    .await;
    let (status, released) = timer_post(
        &fx,
        &fx.cookie,
        "/api/v1/me/task-timer/legacy-release",
        &release,
    )
    .await;
    assert!(status.is_success(), "{status} {released}");
    old.push((
        "legacy-release",
        "/api/v1/me/task-timer/legacy-release".into(),
        release,
    ));

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("every command kind's history is captured");
    let archive = captured.archive;
    let g = &archive.graph;
    let mut requests: Vec<Uuid> = g.timer_commands.iter().map(|c| c.request_id).collect();
    requests.sort();
    let mut expected: Vec<Uuid> = old
        .iter()
        .map(|(_, _, body)| Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap())
        .collect();
    expected.sort();
    assert_eq!(requests, expected, "one receipt per command kind");
    // The self cleanup's audit has no locators; it travels with its receipt.
    let cleanup_request = old
        .iter()
        .find(|(kind, _, _)| *kind == "cleanup")
        .map(|(_, _, body)| Uuid::parse_str(body["requestId"].as_str().unwrap()).unwrap())
        .unwrap();
    assert!(g.timer_audit.iter().any(|a| a.request_id == cleanup_request
        && a.workspace_id.is_none()
        && a.task_id.is_none()
        && a.time_entry_id.is_none()));

    // The same person on a second installation restores the archive.
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let cookie = same_person(&dst, fx.user_id).await;
    let (destination, _, claim) = claimed_restore(&dst, fx.user_id, &cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("task time restores");
    let marked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id=$1 AND restored_from_archive=$2",
    )
    .bind(fx.user_id)
    .bind(claim.job_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(marked, 8);

    // Every old request, retried by its client under a current session, is a
    // conflict with no effect. Workspace-bound hashes name the source
    // workspace (request_mismatch first); /me hashes are equal, so the 052
    // marker retires them before their stored result is used.
    let before = timer_graph(&dst.admin, fx.user_id).await;
    for (kind, path, body) in &old {
        let body = timer_body(&dst, &cookie, body.clone()).await;
        let path = path.replace("{ws}", &destination.to_string());
        let (status, reply) = timer_post(&dst, &cookie, &path, &body).await;
        let code = if path.starts_with("/api/v1/me/") {
            "timer_retired"
        } else {
            "request_mismatch"
        };
        assert_eq!(
            (status, reply["params"]["code"].as_str()),
            (axum::http::StatusCode::CONFLICT, Some(code)),
            "{kind}: {reply}"
        );
        assert_eq!(
            timer_graph(&dst.admin, fx.user_id).await,
            before,
            "{kind} had an effect"
        );
    }

    // Genuinely new requests work normally on the restored tasks.
    let (status, state) = json_request(
        dst.app.clone(),
        "GET",
        &base(destination, tasks[0]),
        None,
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {state}");
    assert_eq!(
        (&state["estimate"]["value"], &state["estimate"]["unit"]),
        (&source_estimate["value"], &source_estimate["unit"])
    );
    let fresh = timer_body(&dst, &cookie, json!({"requestId":Uuid::now_v7(),"expected":state["estimate"],"minutes":45,"reason":"새 견적"})).await;
    let (status, reply) = timer_post(
        &dst,
        &cookie,
        &format!("{}/estimate", base(destination, tasks[0])),
        &fresh,
    )
    .await;
    assert!(status.is_success(), "{status} {reply}");
    let fresh = timer_body(&dst, &cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"새 실행"})).await;
    let (status, started) = timer_post(&dst, &cookie, &base(destination, tasks[1]), &fresh).await;
    assert!(status.is_success(), "{status} {started}");
    let fresh = timer_body(&dst, &cookie, json!({"requestId":Uuid::now_v7(),"runId":started["runId"],"expectedVersion":started["version"]})).await;
    let (status, cleaned) = timer_post(&dst, &cookie, "/api/v1/me/task-timer/stop", &fresh).await;
    assert!(status.is_success(), "{status} {cleaned}");
    assert_eq!(cleaned["status"], "stopped");
    let fresh = timer_body(&dst, &cookie, json!({"requestId":Uuid::now_v7(),"startedAt":"2026-10-03T09:00:00Z","endedAt":"2026-10-03T09:15:00Z","note":null,"reason":"새 수동 기록"})).await;
    let (status, created) = timer_post(
        &dst,
        &cookie,
        &format!("{}/history", base(destination, tasks[0])),
        &fresh,
    )
    .await;
    assert!(status.is_success(), "{status} {created}");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id=$1 AND restored_from_archive IS NULL",
    )
    .bind(fx.user_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(live, 4);

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_preserves_an_unreleased_legacy_reservation() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "LGC", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"열린 기록 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    // An open entry through the ordinary route; its 034 trigger reserves it.
    let (status, open) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}/time-entries"),
        Some(json!({"startedAt":"2026-10-02T12:00:00Z","note":"열린 기록"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {open}");
    let open = Uuid::parse_str(open["id"].as_str().unwrap()).unwrap();
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("an open entry with its reservation is captured");
    let g = &captured.archive.graph;
    assert_eq!(
        g.timer_legacy_open
            .iter()
            .map(|l| (l.time_entry_id, l.task_id))
            .collect::<Vec<_>>(),
        vec![(open, task_id)]
    );
    assert_eq!(
        g.time_entries
            .iter()
            .map(|e| (e.id, e.ended_at.clone()))
            .collect::<Vec<_>>(),
        vec![(open, None)]
    );

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("the open entry restores");
    let restored: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'entries',(SELECT jsonb_agg(jsonb_build_array(id,user_id,ended_at)) FROM fvoci.time_entries WHERE workspace_id=$1),
            'reservations',(SELECT jsonb_agg(jsonb_build_array(time_entry_id,user_id,task_id)) FROM fvoci.task_timer_legacy_open WHERE workspace_id=$1))",
    )
    .bind(destination)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(
        restored,
        json!({"entries":[[open,dst.user_id,null]],"reservations":[[open,dst.user_id,task_id]]})
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_restores_a_released_open_entry_with_a_later_unfinished_run() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "MIX", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let mut tasks = Vec::new();
    for title in ["열린 기록 🧪", "나중 실행"] {
        let (status, task) = json_request(
            fx.app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
            Some(json!({"title":title})),
            Some(&fx.cookie),
        )
        .await;
        assert!(status.is_success(), "{status} {task}");
        tasks.push(Uuid::parse_str(task["id"].as_str().unwrap()).unwrap());
    }
    // The W5 producer shape: an old-API open entry, its explicit release (the
    // 034 row stays open), then a new stopwatch that is still unfinished.
    let (status, open) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{}/time-entries", tasks[0]),
        Some(json!({"startedAt":"2026-10-02T12:00:00Z","note":"열린 기록"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {open}");
    let open_id = Uuid::parse_str(open["id"].as_str().unwrap()).unwrap();
    let release = timer_body(
        &fx,
        &fx.cookie,
        json!({"requestId":Uuid::now_v7(),"timeEntryId":open["id"]}),
    )
    .await;
    let (status, released) = timer_post(
        &fx,
        &fx.cookie,
        "/api/v1/me/task-timer/legacy-release",
        &release,
    )
    .await;
    assert!(status.is_success(), "{status} {released}");
    let timer = format!("/api/v1/workspaces/{ws}/tasks/{}/timer", tasks[1]);
    let start = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":"나중 실행"})).await;
    let (status, started) = timer_post(&fx, &fx.cookie, &timer, &start).await;
    assert!(status.is_success(), "{status} {started}");
    let pause = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"pause","expectedVersion":started["version"],"runId":started["runId"]})).await;
    let (status, paused) = timer_post(&fx, &fx.cookie, &timer, &pause).await;
    assert!(status.is_success(), "{status} {paused}");
    let run = Uuid::parse_str(started["runId"].as_str().unwrap()).unwrap();

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("a released open entry with a later unfinished run is captured");
    // W5 close_segment may project the start..pause interval into a closed
    // 034 row (when it lasted at least a second); the producer's rows, not an
    // elapsed-time guess, are the oracle. The released open row stays open.
    let source = person_time_rows(&fx.admin, ws, fx.user_id).await;
    assert!(
        source["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == json!(open_id) && e["endedUs"].is_null()),
        "{source}"
    );
    assert_eq!(source["reservations"], json!(0), "{source}");
    let g = &captured.archive.graph;
    let mut captured_entries: Vec<Uuid> = g.time_entries.iter().map(|e| e.id).collect();
    captured_entries.sort();
    let source_entries: Vec<Uuid> = source["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Uuid::parse_str(e["id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(captured_entries, source_entries);
    let mut captured_segments: Vec<Uuid> = g.timer_segments.iter().map(|s| s.id).collect();
    captured_segments.sort();
    let source_segments: Vec<Uuid> = source["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| Uuid::parse_str(s["id"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(captured_segments, source_segments);
    assert!(g.timer_legacy_open.is_empty());
    assert_eq!(
        g.timer_runs
            .iter()
            .map(|r| (r.id, r.status.clone()))
            .collect::<Vec<_>>(),
        vec![(run, "paused".to_owned())]
    );

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("the mixed graph restores");
    // Every source entry/segment/run lands with the same identity, task,
    // times, duration and note under the destination person and workspace.
    let restored = person_time_rows(&dst.admin, destination, dst.user_id).await;
    assert_eq!(restored, source);
    assert_eq!(
        restored["runs"],
        json!([{"id":run,"task":tasks[1],"status":"paused","version":paused["version"]}])
    );
    // The restored person's own state agrees: the run is theirs, no legacy.
    let (status, owner) = json_request(
        dst.app.clone(),
        "GET",
        "/api/v1/me/task-timer",
        None,
        Some(&dst.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {owner}");
    assert_eq!(
        (&owner["runId"], &owner["legacyOpen"]),
        (&json!(run), &json!(false))
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

/// One person's 034/048 rows in one workspace, with exact microsecond times
/// and the identities that a restore keeps (person and workspace are the
/// query's own filters, so source and destination compare directly).
async fn person_time_rows(admin: &sqlx::PgPool, workspace: Uuid, user: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'entries',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'task',task_id,
                'startedUs',(extract(epoch FROM started_at)*1000000)::bigint,
                'endedUs',(extract(epoch FROM ended_at)*1000000)::bigint,
                'seconds',duration_seconds,'note',note) ORDER BY id),'[]')
                FROM fvoci.time_entries WHERE workspace_id=$1 AND user_id=$2),
            'segments',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'run',run_id,'task',task_id,
                'startedUs',(extract(epoch FROM started_at)*1000000)::bigint,
                'endedUs',(extract(epoch FROM ended_at)*1000000)::bigint,
                'entry',time_entry_id) ORDER BY id),'[]')
                FROM fvoci.task_timer_segments WHERE workspace_id=$1 AND user_id=$2),
            'runs',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'task',task_id,'status',status,
                'version',version) ORDER BY id),'[]')
                FROM fvoci.task_timer_runs WHERE workspace_id=$1 AND user_id=$2),
            'reservations',(SELECT count(*) FROM fvoci.task_timer_legacy_open WHERE workspace_id=$1 AND user_id=$2))",
    )
    .bind(workspace)
    .bind(user)
    .fetch_one(admin)
    .await
    .unwrap()
}

#[tokio::test]
async fn native_archive_restores_milestones_and_dependencies_for_a_fresh_client() {
    use fvoci_server::db::native_archive::{capture, publish};
    use fvoci_server::native_archive::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let call = |app: axum::Router,
                method: &'static str,
                path: String,
                body: Option<Value>,
                cookie: String| async move {
        let (status, reply) = json_request(app, method, &path, body, Some(&cookie)).await;
        assert!(status.is_success(), "{method} {path}: {status} {reply}");
        reply
    };
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "MIL", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let base = format!("/api/v1/workspaces/{ws}/projects/{project_id}");
    // The ordinary writers: two milestones, a task created on the first, a
    // task moved onto the second (a milestoneId activity), and two edges.
    let first = call(
        fx.app.clone(),
        "POST",
        format!("{base}/milestones"),
        Some(json!({"name":"1차 마감 🧪","dueDate":"2026-10-31"})),
        fx.cookie.clone(),
    )
    .await;
    let second = call(
        fx.app.clone(),
        "POST",
        format!("{base}/milestones"),
        Some(json!({"name":"2차 마감"})),
        fx.cookie.clone(),
    )
    .await;
    let a = call(
        fx.app.clone(),
        "POST",
        format!("{base}/tasks"),
        Some(json!({"title":"설계 🧪","milestoneId":first["id"]})),
        fx.cookie.clone(),
    )
    .await;
    let b = call(
        fx.app.clone(),
        "POST",
        format!("{base}/tasks"),
        Some(json!({"title":"구현"})),
        fx.cookie.clone(),
    )
    .await;
    let c = call(
        fx.app.clone(),
        "POST",
        format!("{base}/tasks"),
        Some(json!({"title":"검증"})),
        fx.cookie.clone(),
    )
    .await;
    let task = |v: &Value| Uuid::parse_str(v["id"].as_str().unwrap()).unwrap();
    let (a_id, b_id, c_id) = (task(&a), task(&b), task(&c));
    call(
        fx.app.clone(),
        "PATCH",
        format!("/api/v1/workspaces/{ws}/tasks/{b_id}"),
        Some(json!({"milestoneId":second["id"]})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "POST",
        format!("/api/v1/workspaces/{ws}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId":b_id})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "POST",
        format!("/api/v1/workspaces/{ws}/tasks/{b_id}/dependencies"),
        Some(json!({"blockedId":c_id,"type":"SS","lagDays":2})),
        fx.cookie.clone(),
    )
    .await;
    let source_milestones = call(
        fx.app.clone(),
        "GET",
        format!("{base}/milestones"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let source_edges = call(
        fx.app.clone(),
        "GET",
        format!("{base}/dependencies"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let source_b_activity = call(
        fx.app.clone(),
        "GET",
        format!("/api/v1/workspaces/{ws}/tasks/{b_id}/activity"),
        None,
        fx.cookie.clone(),
    )
    .await;

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("milestones and dependencies are captured");
    let g = &captured.archive.graph;
    let mut milestone_ids: Vec<Uuid> = g.milestones.iter().map(|m| m.id).collect();
    milestone_ids.sort();
    let mut expected = vec![task(&first), task(&second)];
    expected.sort();
    assert_eq!(milestone_ids, expected);
    let mut edges: Vec<(Uuid, Uuid, String, i32)> = g
        .dependencies
        .iter()
        .map(|d| (d.blocker_id, d.blocked_id, d.r#type.clone(), d.lag_days))
        .collect();
    edges.sort();
    let mut expected = vec![
        (a_id, b_id, "FS".to_owned(), 0),
        (b_id, c_id, "SS".to_owned(), 2),
    ];
    expected.sort();
    assert_eq!(edges, expected);
    let milestone_of = |id: Uuid| g.tasks.iter().find(|t| t.id == id).unwrap().milestone_id;
    assert_eq!(
        (milestone_of(a_id), milestone_of(b_id), milestone_of(c_id)),
        (Some(task(&first)), Some(task(&second)), None)
    );
    assert!(g.activity.iter().any(|act| act.task_id == b_id
        && act
            .changes
            .as_array()
            .is_some_and(|list| list.iter().any(|ch| ch["field"] == "milestoneId"
                && ch["to"] == json!({"id":second["id"],"label":"2차 마감"})))));
    captured
        .archive
        .validate()
        .expect("the archive policy accepts the graph");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("milestones and dependencies restore");
    // A fresh client of the destination sees the same milestones, edges,
    // assignments and history through the ordinary routes.
    let dbase = format!("/api/v1/workspaces/{destination}/projects/{project_id}");
    let restored_milestones = call(
        dst.app.clone(),
        "GET",
        format!("{dbase}/milestones"),
        None,
        dst.cookie.clone(),
    )
    .await;
    let mut expected_milestones = source_milestones.clone();
    for item in expected_milestones["items"].as_array_mut().unwrap() {
        item["projectId"] = json!(project_id);
    }
    assert_eq!(restored_milestones, expected_milestones);
    assert_eq!(
        call(
            dst.app.clone(),
            "GET",
            format!("{dbase}/dependencies"),
            None,
            dst.cookie.clone()
        )
        .await,
        source_edges
    );
    let detail = call(
        dst.app.clone(),
        "GET",
        format!("/api/v1/workspaces/{destination}/tasks/{b_id}"),
        None,
        dst.cookie.clone(),
    )
    .await;
    assert_eq!(detail["milestoneId"], second["id"]);
    assert_eq!(
        detail["dependencies"].as_array().unwrap().len(),
        2,
        "{detail}"
    );
    let restored_b_activity = call(
        dst.app.clone(),
        "GET",
        format!("/api/v1/workspaces/{destination}/tasks/{b_id}/activity"),
        None,
        dst.cookie.clone(),
    )
    .await;
    let changes = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["id"].clone(), i["kind"].clone(), i["changes"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(changes(&restored_b_activity), changes(&source_b_activity));
    // The restored graph is live for the writer: a closing edge is refused as
    // a cycle, and new milestones/edges work.
    let (status, cycle) = json_request(
        dst.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{destination}/tasks/{c_id}/dependencies"),
        Some(json!({"blockedId":a_id})),
        Some(&dst.cookie),
    )
    .await;
    assert_eq!(
        (status, cycle["code"].as_str()),
        (
            axum::http::StatusCode::BAD_REQUEST,
            Some("dependency_cycle")
        ),
        "{cycle}"
    );
    let third = call(
        dst.app.clone(),
        "POST",
        format!("{dbase}/milestones"),
        Some(json!({"name":"3차"})),
        dst.cookie.clone(),
    )
    .await;
    call(
        dst.app.clone(),
        "PATCH",
        format!("/api/v1/workspaces/{destination}/tasks/{c_id}"),
        Some(json!({"milestoneId":third["id"]})),
        dst.cookie.clone(),
    )
    .await;
    call(
        dst.app.clone(),
        "POST",
        format!("/api/v1/workspaces/{destination}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId":c_id,"type":"FF"})),
        dst.cookie.clone(),
    )
    .await;

    // Incomplete history: once a milestone that a task's history names is
    // purged, the history reference has no archived milestone and the export
    // is refused as a typed model, never silently pruned.
    let gone = call(
        fx.app.clone(),
        "POST",
        format!("{base}/milestones"),
        Some(json!({"name":"폐기될 마감"})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PATCH",
        format!("/api/v1/workspaces/{ws}/tasks/{c_id}"),
        Some(json!({"milestoneId":gone["id"]})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "DELETE",
        format!("{base}/milestones/{}", gone["id"].as_str().unwrap()),
        None,
        fx.cookie.clone(),
    )
    .await;
    let refused = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("capture reads the graph")
        .archive
        .validate();
    assert!(
        matches!(&refused, Err(ArchiveError::Unsupported(m)) if m == "non-baseline task activity"),
        "{refused:?}"
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_restores_owner_saved_views_for_a_fresh_client() {
    use fvoci_server::db::native_archive::{capture, publish};
    use fvoci_server::native_archive::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let call = |app: axum::Router,
                method: &'static str,
                path: String,
                body: Option<Value>,
                cookie: String| async move {
        let (status, reply) = json_request(app, method, &path, body, Some(&cookie)).await;
        assert!(status.is_success(), "{method} {path}: {status} {reply}");
        reply
    };
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "VEW", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let base = format!("/api/v1/workspaces/{ws}/projects/{project_id}");
    // The ordinary writers: a label, a milestone, a task (for its status) and
    // two owner views whose filters name them and the owner.
    let label = call(
        fx.app.clone(),
        "POST",
        format!("{base}/labels"),
        Some(json!({"name":"검토 🧪","color":"blue"})),
        fx.cookie.clone(),
    )
    .await;
    let milestone = call(
        fx.app.clone(),
        "POST",
        format!("{base}/milestones"),
        Some(json!({"name":"마감"})),
        fx.cookie.clone(),
    )
    .await;
    let task = call(
        fx.app.clone(),
        "POST",
        format!("{base}/tasks"),
        Some(json!({"title":"보기 대상"})),
        fx.cookie.clone(),
    )
    .await;
    let board = call(
        fx.app.clone(),
        "POST",
        format!("{base}/views"),
        Some(json!({"name":"내 보드 🧪","type":"board","config":{"filters":{"statusId":task["statusId"],
            "labelId":label["id"],"milestoneId":milestone["id"],"assigneeId":fx.user_id.to_string(),"openOnly":true},
            "sort":[{"field":"due","direction":"asc"}]}})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "POST",
        format!("{base}/views"),
        Some(json!({"name":"내 목록","type":"list","config":{"filters":{"assigneeId":"me"}}})),
        fx.cookie.clone(),
    )
    .await;
    let source_views = call(
        fx.app.clone(),
        "GET",
        format!("{base}/views"),
        None,
        fx.cookie.clone(),
    )
    .await;

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("saved views are captured");
    let mut ids: Vec<String> = captured
        .archive
        .graph
        .views
        .iter()
        .map(|v| v.id.to_string())
        .collect();
    ids.sort();
    let mut expected: Vec<String> = source_views["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    expected.sort();
    assert_eq!(ids, expected);
    captured
        .archive
        .validate()
        .expect("the archive policy accepts the views");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("saved views restore");
    // The destination owner's fresh client lists the same views; an assignee
    // filter that named the source owner names the destination owner, "me"
    // stays "me".
    let dbase = format!("/api/v1/workspaces/{destination}/projects/{project_id}");
    let restored = call(
        dst.app.clone(),
        "GET",
        format!("{dbase}/views"),
        None,
        dst.cookie.clone(),
    )
    .await;
    let mut expected = source_views.clone();
    for item in expected["items"].as_array_mut().unwrap() {
        if item["config"]["filters"]["assigneeId"] == json!(fx.user_id.to_string()) {
            item["config"]["filters"]["assigneeId"] = json!(dst.user_id.to_string());
        }
    }
    assert_eq!(restored, expected);
    // The restored view is live for its writer: a compare-and-swap config
    // change from the restored config succeeds.
    let board_id = board["id"].as_str().unwrap();
    let restored_board = restored["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == board["id"])
        .unwrap()
        .clone();
    let changed = call(
        dst.app.clone(),
        "PATCH",
        format!("/api/v1/workspaces/{destination}/views/{board_id}"),
        Some(json!({"name":"복원된 보드","config":{"filters":{"openOnly":true},"sort":[]},"expectedConfig":restored_board["config"]})),
        dst.cookie.clone(),
    )
    .await;
    // The writer answers {ok}; a fresh read shows the change.
    assert_eq!(changed, json!({"ok":true}));
    let reread = call(
        dst.app.clone(),
        "GET",
        format!("{dbase}/views"),
        None,
        dst.cookie.clone(),
    )
    .await;
    let reread_board = reread["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == board["id"])
        .unwrap()
        .clone();
    assert_eq!(
        (&reread_board["name"], &reread_board["config"]),
        (
            &json!("복원된 보드"),
            &json!({"filters":{"openOnly":true},"sort":[]})
        )
    );

    // Another person's private view of the project is refused, never left
    // behind (fixture writer: the admin role inserts that member's row).
    let other = project_harness::add_workspace_user(&fx.admin, ws, "member", "view-owner").await;
    sqlx::query("INSERT INTO fvoci.views (id, workspace_id, project_id, user_id, name, type, config) VALUES ($1,$2,$3,$4,'남의 보기','list','{\"filters\":{},\"sort\":[]}')")
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(project_id)
        .bind(other.user_id)
        .execute(&fx.admin)
        .await
        .unwrap();
    let refused = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("capture reads every view")
        .archive
        .validate();
    assert!(
        matches!(&refused, Err(ArchiveError::Unsupported(m)) if m == "views"),
        "{refused:?}"
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_keeps_historical_audit_locators_and_refuses_retained_provenance() {
    use fvoci_server::db::native_archive::{capture, publish, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "HIS", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"이동된 태스크 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let start = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":null})).await;
    let timer = format!("/api/v1/workspaces/{ws}/tasks/{task_id}/timer");
    let (status, started) = timer_post(&fx, &fx.cookie, &timer, &start).await;
    assert!(status.is_success(), "{status} {started}");
    let stop = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"stop","expectedVersion":started["version"],"runId":started["runId"]})).await;
    let (status, stopped) = timer_post(&fx, &fx.cookie, &timer, &stop).await;
    assert!(status.is_success(), "{status} {stopped}");
    // The MOVE provenance shape (labeled restricted-role fixture writer: W2
    // MOVE is not in this tree): an audit of this task written while it lived
    // in an earlier workspace keeps that historical workspace locator.
    let (historical, earlier) = (Uuid::now_v7(), Uuid::now_v7());
    as_actor(&fx.pool, ws, fx.user_id,
        "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at)
         SELECT (b->>0)::uuid,(b->>1)::uuid,gen_random_uuid(),(b->>2)::uuid,(b->>3)::uuid,NULL,'task.estimate.minutes','null'::jsonb,jsonb_build_object('value','30','unit','minutes'),'이전 워크스페이스','2026-10-01T09:00:00Z' FROM (SELECT $1::jsonb AS b) q",
        vec![json!(historical), json!(fx.user_id), json!(earlier), json!(task_id)]).await;
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("historical provenance is captured");
    let archive = captured.archive;
    assert!(archive
        .graph
        .timer_audit
        .iter()
        .any(|a| a.id == historical && a.workspace_id == Some(earlier)));
    archive
        .validate()
        .expect("a historical locator with a selected task is accepted");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("historical provenance restores");
    let locators: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, workspace_id FROM fvoci.task_timer_audit WHERE user_id=$1 ORDER BY created_at, id",
    )
    .bind(dst.user_id)
    .fetch_all(&dst.admin)
    .await
    .unwrap();
    let mut expected: Vec<(Uuid, Option<Uuid>)> = archive
        .graph
        .timer_audit
        .iter()
        .map(|a| {
            (
                a.id,
                a.workspace_id
                    .map(|w| if w == ws { destination } else { w }),
            )
        })
        .collect();
    expected.sort_by_key(|(id, _)| *id);
    let mut got = locators.clone();
    got.sort_by_key(|(id, _)| *id);
    assert_eq!(got, expected);
    assert!(
        got.contains(&(historical, Some(earlier))),
        "historical locator unchanged"
    );

    // A real purge keeps this actor's receipts and audit naming the task.
    let (status, purged) = json_request(
        fx.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}"),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {purged}");
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_timer_audit WHERE user_id=$1 AND task_id=$2",
    )
    .bind(fx.user_id)
    .bind(task_id)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    assert!(retained >= 3, "{retained}");

    // ABA: the same person on another installation already retains history
    // naming these identities (the post-purge shape, labeled fixture writer).
    // Restoring the same IDs is refused before effects, also when the archive
    // omits its receipts and audit.
    let aba_db = TestDb::bootstrap().await;
    let aba = fixture(&aba_db).await;
    let cookie = same_person(&aba, fx.user_id).await;
    as_actor(&aba.pool, aba.workspace_id, fx.user_id,
        "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at)
         SELECT gen_random_uuid(),(b->>0)::uuid,gen_random_uuid(),NULL,(b->>1)::uuid,NULL,'running','{}'::jsonb,'{}'::jsonb,'남은 기록',now() FROM (SELECT $1::jsonb AS b) q",
        vec![json!(fx.user_id), json!(task_id)]).await;
    let mut omitted = archive.clone();
    omitted.graph.timer_commands.clear();
    omitted.graph.timer_audit.clear();
    omitted
        .validate()
        .expect("an archive may omit receipts and audit");
    // One claimed restore; each refused publish rolls back entirely.
    let (target, _, claim) = claimed_restore(&aba, fx.user_id, &cookie).await;
    for (label, candidate) in [("full", &archive), ("omitted", &omitted)] {
        let refused = publish(
            &aba.pool,
            &claim,
            candidate,
            &std::collections::BTreeMap::new(),
            &aba.settings.quota,
        )
        .await;
        assert!(
            matches!(refused, Err(NativeDbError::Conflict)),
            "{label}: {refused:?}"
        );
        let effects: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1)+(SELECT count(*) FROM fvoci.task_timer_runs WHERE user_id=$2)+(SELECT count(*) FROM fvoci.task_timer_commands WHERE user_id=$2)",
        )
        .bind(target)
        .bind(fx.user_id)
        .fetch_one(&aba.admin)
        .await
        .unwrap();
        assert_eq!(effects, 0, "{label}");
    }

    let storages = [fx.storage_root(), dst.storage_root(), aba.storage_root()];
    for f in [&fx, &dst, &aba] {
        f.pool.close().await;
        f.admin.close().await;
    }
    harness.cleanup().await;
    destination_db.cleanup().await;
    aba_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

/// A captured single-author archive with task time from the real routes: a
/// closed manual entry and a stopped run with its segment.
async fn timed_archive() -> (TestDb, Fixture, fvoci_server::native_archive::Archive) {
    use fvoci_server::db::native_archive::capture;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "GRD", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"보호 대상 🧪"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let task_id = task["id"].as_str().unwrap().to_owned();
    let (status, entry) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{task_id}/time-entries"),
        Some(json!({"startedAt":"2026-10-02T09:00:00.000Z","endedAt":"2026-10-02T10:00:00.000Z"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {entry}");
    let timer = format!("/api/v1/workspaces/{ws}/tasks/{task_id}/timer");
    let start = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"start","expectedVersion":0,"runId":null,"note":null})).await;
    let (status, started) = timer_post(&fx, &fx.cookie, &timer, &start).await;
    assert!(status.is_success(), "{status} {started}");
    let stop = timer_body(&fx, &fx.cookie, json!({"requestId":Uuid::now_v7(),"operation":"stop","expectedVersion":started["version"],"runId":started["runId"]})).await;
    let (status, stopped) = timer_post(&fx, &fx.cookie, &timer, &stop).await;
    assert!(status.is_success(), "{status} {stopped}");
    let archive = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("timed archive")
        .archive;
    archive.validate().expect("timed archive is valid");
    assert!(!archive.graph.timer_segments.is_empty() && !archive.graph.time_entries.is_empty());
    (harness, fx, archive)
}

/// Digest every row of every application table, including jobs, outbox and
/// events. Only digests leave this oracle; credential values are not logged.
async fn restore_database_effects(admin: &sqlx::PgPool) -> Vec<(String, i64, String)> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname='fvoci' ORDER BY tablename",
    )
    .fetch_all(admin)
    .await
    .unwrap();
    assert!(!tables.is_empty());
    let mut effects = Vec::new();
    for table in tables {
        let quoted = table.replace('"', "\"\"");
        let (count, digest): (i64, String) = sqlx::query_as(&format!(
            "SELECT count(*),coalesce(md5(string_agg(row_json::text,E'\\n' ORDER BY row_json::text)),md5(''))
             FROM (SELECT to_jsonb(t) AS row_json FROM fvoci.\"{quoted}\" t) q"
        ))
        .fetch_one(admin)
        .await
        .unwrap();
        effects.push((table, count, digest));
    }
    effects
}

fn restore_storage_effects(root: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    use sha2::{Digest, Sha256};
    fn visit(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<(std::path::PathBuf, String)>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                out.push((relative, "directory".into()));
                visit(root, &path, out);
            } else {
                out.push((
                    relative,
                    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                ));
            }
        }
    }
    let mut effects = Vec::new();
    visit(root, root, &mut effects);
    effects.sort();
    effects
}

/// Literal publisher graph plus allocator, job/result and event rows. The
/// all-table digest separately covers every other table without logging secrets.
async fn restore_publication_graph(admin: &sqlx::PgPool) -> Value {
    let tables = [
        "workspaces",
        "projects",
        "project_members",
        "documents",
        "tasks",
        "collections",
        "collection_items",
        "workflows",
        "statuses",
        "milestones",
        "task_assignees",
        "task_dependencies",
        "views",
        "labels",
        "task_labels",
        "task_origins",
        "task_activity",
        "comments",
        "document_states",
        "task_states",
        "document_collab_updates",
        "task_collab_updates",
        "document_collab_op_receipts",
        "task_collab_op_receipts",
        "revisions",
        "attachments",
        "attachment_object_cleanups",
        "zotero_connectors",
        "zotero_collections",
        "zotero_references",
        "zotero_memberships",
        "zotero_links",
        "time_entries",
        "task_timer_runs",
        "task_timer_segments",
        "task_timer_legacy_open",
        "task_timer_commands",
        "task_timer_audit",
        "personal_input_commands",
        "import_jobs",
        "events",
    ];
    let mut graph = serde_json::Map::new();
    for table in tables {
        let rows: Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb)
             FROM fvoci.{table} t"
        ))
        .fetch_one(admin)
        .await
        .unwrap();
        graph.insert(table.into(), rows);
    }
    Value::Object(graph)
}

#[tokio::test]
async fn native_archive_post053_command_only_guard_refuses_full_and_omitted_history() {
    use fvoci_server::db::native_archive::{publish, NativeDbError};
    let (harness, fx, archive) = timed_archive().await;
    let versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations ORDER BY version")
            .fetch_all(&fx.admin)
            .await
            .unwrap();
    assert_eq!(versions, (1..=54).collect::<Vec<_>>());
    let run = archive.graph.timer_runs[0].id;
    let task = archive.graph.timer_runs[0].task_id;
    let historical: Value = sqlx::query_scalar(
        "SELECT to_jsonb(c) FROM fvoci.task_timer_commands c
         WHERE user_id=$1 AND run_id=$2 ORDER BY request_id LIMIT 1",
    )
    .bind(fx.user_id)
    .bind(run)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    let history = timer_graph(&fx.admin, fx.user_id).await;
    // Real command writer followed by ordinary purge: 053 must retain the
    // exact request UUID, hash, result and run locator after canonical loss.
    let (status, body) = json_request(
        fx.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/tasks/{task}", fx.workspace_id),
        None,
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let canonical: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.task_timer_runs WHERE id=$1")
            .bind(run)
            .fetch_one(&fx.admin)
            .await
            .unwrap();
    assert_eq!(canonical, 0);
    let purged = timer_graph(&fx.admin, fx.user_id).await;
    assert_eq!(purged["commands"], history["commands"]);
    assert_eq!(purged["audit"], history["audit"]);
    let mut omitted = archive.clone();
    omitted.graph.timer_commands.clear();
    omitted.graph.timer_audit.clear();
    omitted.validate().unwrap();
    assert!(archive
        .graph
        .timer_commands
        .iter()
        .any(|c| c.run_id == Some(run)));
    assert!(!archive.graph.timer_audit.is_empty());

    for (label, foreign, different_run) in [
        ("same actor", false, false),
        ("foreign actor", true, false),
        ("different run", false, true),
    ] {
        for (history_label, candidate) in [("full", &archive), ("omitted", &omitted)] {
            let db = TestDb::bootstrap().await;
            let inst = fixture(&db).await;
            let cookie = same_person(&inst, fx.user_id).await;
            let owner = if foreign {
                project_harness::add_workspace_user(
                    &inst.admin,
                    inst.workspace_id,
                    "member",
                    "command-other",
                )
                .await
                .user_id
            } else {
                fx.user_id
            };
            let missing_fk: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_constraint
             WHERE conrelid='fvoci.task_timer_commands'::regclass
               AND conname='task_timer_commands_run_id_fkey'",
            )
            .fetch_one(&inst.admin)
            .await
            .unwrap();
            assert_eq!(missing_fk, 0, "post053 fixture required");
            // A labeled history-only fixture on a second installation. The
            // negative uses the exact real post-purge command, with NO audit or
            // canonical state. Positive controls change only its owner or its
            // request/run identities (and matching result runId).
            let mut retained = historical.clone();
            retained["user_id"] = json!(owner);
            if different_run {
                let unrelated = Uuid::now_v7();
                retained["request_id"] = json!(Uuid::now_v7());
                retained["run_id"] = json!(unrelated);
                retained["result"]["runId"] = json!(unrelated);
            }
            as_actor(&inst.pool, inst.workspace_id, owner,
            "INSERT INTO fvoci.task_timer_commands(user_id,request_id,request_hash,run_id,result,created_at,restored_from_archive)
             SELECT (b->>'user_id')::uuid,(b->>'request_id')::uuid,b->>'request_hash',(b->>'run_id')::uuid,b->'result',
                    (b->>'created_at')::timestamptz,(b->>'restored_from_archive')::uuid
             FROM (SELECT $1::jsonb->0 AS b) q",
            vec![retained.clone()]).await;
            let request = Uuid::parse_str(retained["request_id"].as_str().unwrap()).unwrap();
            let read_command = || async {
                sqlx::query_scalar::<_, Value>(
                "SELECT to_jsonb(c) FROM fvoci.task_timer_commands c WHERE user_id=$1 AND request_id=$2",
            )
            .bind(owner)
            .bind(request)
            .fetch_one(&inst.admin)
            .await
            .unwrap()
            };
            assert_eq!(read_command().await, retained);
            let before_timer = timer_graph(&inst.admin, owner).await;
            assert!(before_timer["runs"].as_array().unwrap().is_empty());
            assert!(before_timer["audit"].as_array().unwrap().is_empty());
            let (target, _, claim) = claimed_restore(&inst, fx.user_id, &cookie).await;
            let before_db = restore_database_effects(&inst.admin).await;
            let before_graph = restore_publication_graph(&inst.admin).await;
            let before_storage = restore_storage_effects(&inst.storage_root());
            let result = publish(
                &inst.pool,
                &claim,
                candidate,
                &std::collections::BTreeMap::new(),
                &inst.settings.quota,
            )
            .await;
            if foreign || different_run {
                result.unwrap_or_else(|e| panic!("{label}/{history_label}: {e:?}"));
                let restored: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM fvoci.task_timer_runs WHERE id=$1 AND user_id=$2",
                )
                .bind(run)
                .bind(fx.user_id)
                .fetch_one(&inst.admin)
                .await
                .unwrap();
                assert_eq!(restored, 1, "{label}");
                let projects: i64 =
                    sqlx::query_scalar("SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1")
                        .bind(target)
                        .fetch_one(&inst.admin)
                        .await
                        .unwrap();
                assert_eq!(projects, 1, "{label}");
            } else {
                assert!(
                    matches!(result, Err(NativeDbError::Conflict)),
                    "{history_label}: {result:?}"
                );
                assert_eq!(
                    restore_database_effects(&inst.admin).await,
                    before_db,
                    "{history_label}: all rows/jobs/events unchanged"
                );
                assert_eq!(
                    restore_publication_graph(&inst.admin).await,
                    before_graph,
                    "{history_label}: literal publication graph/jobs/events unchanged"
                );
                assert_eq!(
                    restore_storage_effects(&inst.storage_root()),
                    before_storage,
                    "{history_label}: owned storage unchanged"
                );
                assert_eq!(
                    timer_graph(&inst.admin, owner).await,
                    before_timer,
                    "{history_label}: exact retained history"
                );
            }
            assert_eq!(
                read_command().await,
                retained,
                "{label}/{history_label}: immutable receipt"
            );
            let storage = inst.storage_root();
            inst.pool.close().await;
            inst.admin.close().await;
            db.cleanup().await;
            std::fs::remove_dir_all(storage).unwrap();
        }
    }
    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_retained_task_guard_blocks_timeless_before_early_return() {
    use fvoci_server::db::native_archive::{publish, NativeDbError};
    let (harness, fx, mut timeless) = timed_archive().await;
    let task = timeless.graph.timer_runs[0].task_id;
    let g = &mut timeless.graph;
    (
        g.time_entries,
        g.timer_runs,
        g.timer_segments,
        g.timer_legacy_open,
        g.timer_commands,
        g.timer_audit,
    ) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    timeless.validate().unwrap();
    assert!(timeless.graph.tasks.iter().any(|t| t.id == task));
    // All time arrays are empty: without the retained selected-task guard
    // this publication would take the no-time early return and commit.
    for (label, foreign, unrelated) in [
        ("selected task", false, false),
        ("foreign actor", true, false),
        ("unrelated task", false, true),
    ] {
        let db = TestDb::bootstrap().await;
        let inst = fixture(&db).await;
        let cookie = same_person(&inst, fx.user_id).await;
        let owner = if foreign {
            project_harness::add_workspace_user(
                &inst.admin,
                inst.workspace_id,
                "member",
                "timeless-other",
            )
            .await
            .user_id
        } else {
            fx.user_id
        };
        let retained_task = if unrelated { Uuid::now_v7() } else { task };
        as_actor(&inst.pool, inst.workspace_id, owner,
            "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason)
             SELECT gen_random_uuid(),(b->>0)::uuid,gen_random_uuid(),NULL,(b->>1)::uuid,NULL,'retained','{}'::jsonb,'{}'::jsonb,'남은 기록' FROM (SELECT $1::jsonb AS b) q",
            vec![json!(owner), json!(retained_task)]).await;
        let (target, _, claim) = claimed_restore(&inst, fx.user_id, &cookie).await;
        let before_db = restore_database_effects(&inst.admin).await;
        let before_graph = restore_publication_graph(&inst.admin).await;
        let before_timer = timer_graph(&inst.admin, owner).await;
        let before_storage = restore_storage_effects(&inst.storage_root());
        let result = publish(
            &inst.pool,
            &claim,
            &timeless,
            &std::collections::BTreeMap::new(),
            &inst.settings.quota,
        )
        .await;
        if foreign || unrelated {
            result.unwrap_or_else(|e| panic!("{label}: {e:?}"));
            let projects: i64 =
                sqlx::query_scalar("SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1")
                    .bind(target)
                    .fetch_one(&inst.admin)
                    .await
                    .unwrap();
            assert_eq!(projects, 1, "{label}");
            let history = timer_graph(&inst.admin, owner).await;
            assert_eq!(history["audit"], before_timer["audit"]);
            assert_eq!(history["commands"], before_timer["commands"]);
            assert!(history["runs"].as_array().unwrap().is_empty());
        } else {
            assert!(
                matches!(result, Err(NativeDbError::Conflict)),
                "{label}: {result:?}"
            );
            assert_eq!(restore_database_effects(&inst.admin).await, before_db);
            assert_eq!(restore_publication_graph(&inst.admin).await, before_graph);
            assert_eq!(timer_graph(&inst.admin, owner).await, before_timer);
            assert_eq!(
                restore_storage_effects(&inst.storage_root()),
                before_storage
            );
        }
        let storage = inst.storage_root();
        inst.pool.close().await;
        inst.admin.close().await;
        db.cleanup().await;
        std::fs::remove_dir_all(storage).unwrap();
    }
    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

/// A retained timer audit row: task, entry, before and after values.
type RetainedAudit = (Option<Uuid>, Option<Uuid>, Value, Value);

/// A fresh installation where `owner` (the archive's person, or another
/// member when `foreign`) already retains the given timer audit rows; then
/// the archive's person restores each candidate on one claim. Returns each
/// result with the restore workspace's project count after it.
async fn restore_over_retained(
    archive_person: Uuid,
    retained: &[RetainedAudit],
    foreign: bool,
    candidates: &[&fvoci_server::native_archive::Archive],
) -> Vec<(bool, String, i64)> {
    use fvoci_server::db::native_archive::publish;
    let db = TestDb::bootstrap().await;
    let inst = fixture(&db).await;
    let cookie = same_person(&inst, archive_person).await;
    let owner = if foreign {
        project_harness::add_workspace_user(&inst.admin, inst.workspace_id, "member", "other-timer")
            .await
            .user_id
    } else {
        archive_person
    };
    // Labeled fixture writer: the restricted role inserts the owner's
    // retained append-only audit rows (task, entry, before, after).
    for (task, entry, before, after) in retained {
        as_actor(&inst.pool, inst.workspace_id, owner,
            "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason)
             SELECT gen_random_uuid(),(b->>0)::uuid,gen_random_uuid(),NULL,(b->>1)::uuid,(b->>2)::uuid,'retained',b->3,b->4,'남은 기록' FROM (SELECT $1::jsonb AS b) q",
            vec![json!(owner), json!(task), json!(entry), before.clone(), after.clone()]).await;
    }
    let (target, _, claim) = claimed_restore(&inst, archive_person, &cookie).await;
    let mut out = Vec::new();
    for candidate in candidates {
        let result = publish(
            &inst.pool,
            &claim,
            candidate,
            &std::collections::BTreeMap::new(),
            &inst.settings.quota,
        )
        .await;
        let projects: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1")
                .bind(target)
                .fetch_one(&inst.admin)
                .await
                .unwrap();
        out.push((result.is_ok(), format!("{:?}", result.err()), projects));
    }
    let storage = inst.storage_root();
    inst.pool.close().await;
    inst.admin.close().await;
    db.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
    out
}

#[tokio::test]
async fn native_archive_retained_guard_ignores_a_wrong_kind_record_id() {
    let (harness, fx, archive) = timed_archive().await;
    // A retained segment-kind audit whose recordId equals a restored *entry*
    // id names no restored segment (IDs are only unique per table): the
    // restore must proceed.
    let entry = archive.graph.time_entries[0].id;
    let wrong_kind = json!({"recordId":entry,"kind":"segment"});
    let results = restore_over_retained(
        fx.user_id,
        &[(None, None, json!({}), wrong_kind)],
        false,
        &[&archive],
    )
    .await;
    assert_eq!(results.len(), 1);
    assert!(results[0].0, "wrong kind: {}", results[0].1);
    assert_eq!(results[0].2, 1);
    // The inverse: a manual-kind recordId equal to a restored segment id.
    let segment = archive.graph.timer_segments[0].id;
    let inverse = json!({"recordId":segment,"kind":"manual"});
    let results = restore_over_retained(
        fx.user_id,
        &[(None, None, inverse, json!({}))],
        false,
        &[&archive],
    )
    .await;
    assert!(results[0].0, "inverse wrong kind: {}", results[0].1);
    assert_eq!(results[0].2, 1);
    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_retained_guard_refuses_each_typed_branch_and_passes_unrelated() {
    let (harness, fx, archive) = timed_archive().await;
    let g = &archive.graph;
    let (task, entry, segment, run) = (
        g.tasks[0].id,
        g.time_entries
            .iter()
            .find(|e| {
                !g.timer_segments
                    .iter()
                    .any(|s| s.time_entry_id == Some(e.id))
            })
            .unwrap()
            .id,
        g.timer_segments[0].id,
        g.timer_runs[0].id,
    );
    let mut omitted = archive.clone();
    omitted.graph.timer_commands.clear();
    omitted.graph.timer_audit.clear();
    omitted.validate().unwrap();
    let both = [&archive, &omitted];
    // Each retained row kind alone refuses the full and the omitted archive
    // with no effects. (A command-only run locator cannot exist before the
    // W5 053 FK change - purge sets it NULL - so that branch stays a W5
    // integration witness.)
    let refusing: Vec<(&str, RetainedAudit)> = vec![
        ("entry column", (None, Some(entry), json!({}), json!({}))),
        (
            "manual before",
            (
                None,
                None,
                json!({"recordId":entry,"kind":"manual"}),
                json!({}),
            ),
        ),
        (
            "segment before",
            (
                None,
                None,
                json!({"recordId":segment,"kind":"segment"}),
                json!({}),
            ),
        ),
        (
            "segment after",
            (
                None,
                None,
                json!({}),
                json!({"recordId":segment,"kind":"segment"}),
            ),
        ),
        (
            "cleanup runId, NULL locators",
            (
                None,
                None,
                json!({"version":1}),
                json!({"runId":run,"recordId":Uuid::now_v7(),"kind":"segment"}),
            ),
        ),
        ("task column", (Some(task), None, json!({}), json!({}))),
        (
            "manual after",
            (
                None,
                None,
                json!({}),
                json!({"recordId":entry,"kind":"manual"}),
            ),
        ),
        (
            "runId before",
            (None, None, json!({"runId":run}), json!({})),
        ),
    ];
    for (label, row) in refusing {
        let results = restore_over_retained(fx.user_id, &[row], false, &both).await;
        for (ok, error, projects) in &results {
            assert!(
                !ok && error.contains("Conflict") && *projects == 0,
                "{label}: {results:?}"
            );
        }
    }
    // Unrelated retained ids, another member's history naming the task, and
    // an archive without task time over unrelated history all restore.
    let unrelated = (
        Some(Uuid::now_v7()),
        Some(Uuid::now_v7()),
        json!({"recordId":Uuid::now_v7(),"kind":"manual"}),
        json!({"runId":Uuid::now_v7()}),
    );
    let results = restore_over_retained(
        fx.user_id,
        std::slice::from_ref(&unrelated),
        false,
        &[&archive],
    )
    .await;
    assert!(results[0].0 && results[0].2 == 1, "unrelated: {results:?}");
    let results = restore_over_retained(
        fx.user_id,
        &[(Some(task), Some(entry), json!({}), json!({}))],
        true,
        &[&archive],
    )
    .await;
    assert!(
        results[0].0 && results[0].2 == 1,
        "foreign actor: {results:?}"
    );
    let mut timeless = archive.clone();
    let tg = &mut timeless.graph;
    (
        tg.time_entries,
        tg.timer_runs,
        tg.timer_segments,
        tg.timer_legacy_open,
        tg.timer_commands,
        tg.timer_audit,
    ) = (vec![], vec![], vec![], vec![], vec![], vec![]);
    timeless.validate().unwrap();
    let results = restore_over_retained(fx.user_id, &[unrelated], false, &[&timeless]).await;
    assert!(
        results[0].0 && results[0].2 == 1,
        "no task time: {results:?}"
    );

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_retained_guard_cost_is_measured() {
    use fvoci_server::db::native_archive::RETAINED_PROVENANCE_SQL;
    let harness = TestDb::bootstrap_through(53).await;
    let fx = fixture(&harness).await;
    let other =
        project_harness::add_workspace_user(&fx.admin, fx.workspace_id, "member", "cost-other")
            .await;
    let fxr = &fx;
    let fill = |owner: Uuid, n: i64| async move {
        as_actor(&fxr.pool, fxr.workspace_id, owner,
            "INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason)
             SELECT gen_random_uuid(),(b->>0)::uuid,gen_random_uuid(),NULL,gen_random_uuid(),NULL,'retained','{}'::jsonb,
                    jsonb_build_object('recordId',gen_random_uuid(),'kind','manual'),'측정' FROM (SELECT $1::jsonb AS b) q, generate_series(1,(b->>1)::int)",
            vec![json!(owner), json!(n)]).await;
    };
    // 100 restored ids of each kind, none retained.
    let ids: Vec<Uuid> = (0..100).map(|_| Uuid::now_v7()).collect();
    let texts: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    let measure = |label: &'static str| {
        let (ids, texts) = (ids.clone(), texts.clone());
        let fx = &fx;
        async move {
            let mut tx = fx.pool.begin().await.unwrap();
            sqlx::query("ANALYZE fvoci.task_timer_audit")
                .execute(&fx.admin)
                .await
                .unwrap();
            fvoci_server::db::context::set_tenant(&mut tx, fx.workspace_id)
                .await
                .unwrap();
            fvoci_server::db::context::set_self_user(&mut tx, fx.user_id)
                .await
                .unwrap();
            let plan: Value = sqlx::query_scalar(&format!(
                "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {RETAINED_PROVENANCE_SQL}"
            ))
            .bind(fx.user_id)
            .bind(&ids)
            .bind(&ids)
            .bind(&ids)
            .bind(&texts)
            .bind(&texts)
            .bind(&texts)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            let found: bool = sqlx::query_scalar(RETAINED_PROVENANCE_SQL)
                .bind(fx.user_id)
                .bind(&ids)
                .bind(&ids)
                .bind(&ids)
                .bind(&texts)
                .bind(&texts)
                .bind(&texts)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            assert!(!found, "{label}: unrelated history must not match");
            // Every plan node over the audit table, as measured.
            fn nodes(v: &Value, out: &mut Vec<Value>) {
                if v.get("Relation Name") == Some(&json!("task_timer_audit")) {
                    out.push(json!({"node":v["Node Type"],"actualRows":v["Actual Rows"],"loops":v["Actual Loops"],
                        "removedByFilter":v["Rows Removed by Filter"],"sharedHit":v["Shared Hit Blocks"],"sharedRead":v["Shared Read Blocks"],
                        "indexName":v["Index Name"],"indexCondition":v["Index Cond"],"recheckCondition":v["Recheck Cond"]}));
                }
                for child in v
                    .get("Plans")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    nodes(child, out);
                }
            }
            let mut audit = Vec::new();
            nodes(&plan[0]["Plan"], &mut audit);
            println!("W8-GUARD-RAW-PLAN {}", json!({"label":label,"plan":plan}));
            println!(
                "W7-GUARD-COST {}",
                json!({"label":label,"executionMs":plan[0]["Execution Time"],"auditNodes":audit})
            );
            assert!(!audit.is_empty(), "{label}: {plan}");
            if label == "actor 20000, other 20000" {
                fn actor_index(v: &Value) -> bool {
                    (v["Index Name"] == "task_timer_audit_user_id_idx"
                        && v["Index Cond"]
                            .as_str()
                            .is_some_and(|s| s.contains("user_id")))
                        || v.get("Plans")
                            .and_then(Value::as_array)
                            .is_some_and(|children| children.iter().any(actor_index))
                }
                assert!(actor_index(&plan[0]["Plan"]), "40k mixed audit: {plan}");
                assert!(
                    audit.iter().any(|a| a["removedByFilter"] == 20_000),
                    "20k own rows filtered; foreign tuples excluded: {audit:?}"
                );
            }
        }
    };
    fill(fx.user_id, 2048).await;
    // Current populated 053 -> 054: the only change is the scalar index.
    // Immutable timer history and every data table retain their fingerprints;
    // FK, RLS/policy, trigger and app-role grant catalogs remain identical.
    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    let security_sql = "SELECT jsonb_build_object(
        'fks',(SELECT jsonb_agg(jsonb_build_object('table',conrelid::regclass::text,'name',conname,'def',pg_get_constraintdef(oid)) ORDER BY conrelid,conname) FROM pg_constraint WHERE connamespace='fvoci'::regnamespace AND contype='f'),
        'policies',(SELECT jsonb_agg(to_jsonb(p) ORDER BY tablename,policyname) FROM pg_policies p WHERE schemaname='fvoci'),
        'triggers',(SELECT jsonb_agg(jsonb_build_object('table',tgrelid::regclass::text,'name',tgname,'def',pg_get_triggerdef(oid)) ORDER BY tgrelid,tgname) FROM pg_trigger WHERE NOT tgisinternal AND tgrelid IN(SELECT oid FROM pg_class WHERE relnamespace='fvoci'::regnamespace)),
        'grants',(SELECT jsonb_agg(to_jsonb(g) ORDER BY table_name,privilege_type) FROM information_schema.role_table_grants g WHERE table_schema='fvoci' AND grantee=$1),
        'rls',(SELECT jsonb_agg(jsonb_build_object('table',relname,'rls',relrowsecurity,'force',relforcerowsecurity,'owner',pg_get_userbyid(relowner)) ORDER BY relname) FROM pg_class WHERE relnamespace='fvoci'::regnamespace AND relkind='r'))";
    let before_security: Value = sqlx::query_scalar(security_sql)
        .bind(&role)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    let before_data = restore_database_effects(&fx.admin).await;
    let migration_metadata_sql =
        "SELECT jsonb_agg(to_jsonb(m) ORDER BY version) FROM fvoci.schema_migrations m";
    let before_migration_metadata: Value = sqlx::query_scalar(migration_metadata_sql)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    let before_history = timer_graph(&fx.admin, fx.user_id).await;
    let before_versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations ORDER BY version")
            .fetch_all(&fx.admin)
            .await
            .unwrap();
    assert_eq!(before_versions, (1..=53).collect::<Vec<_>>());
    let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relnamespace='fvoci'::regnamespace AND relname='task_timer_audit_user_id_idx'")
        .fetch_one(&fx.admin).await.unwrap();
    assert_eq!(absent, 0);
    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .unwrap();
    fvoci_server::db::migrate::assert_schema_current(&fx.admin)
        .await
        .unwrap();
    fvoci_server::db::migrate::assert_app_role(&fx.pool)
        .await
        .unwrap();
    let after_security: Value = sqlx::query_scalar(security_sql)
        .bind(&role)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(after_security, before_security);
    let without_versions = |rows: Vec<(String, i64, String)>| {
        rows.into_iter()
            .filter(|row| row.0 != "schema_migrations")
            .collect::<Vec<_>>()
    };
    let after_data = restore_database_effects(&fx.admin).await;
    assert_eq!(
        without_versions(after_data.clone()),
        without_versions(before_data.clone())
    );
    assert_eq!(timer_graph(&fx.admin, fx.user_id).await, before_history);
    let versions: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations ORDER BY version")
            .fetch_all(&fx.admin)
            .await
            .unwrap();
    assert_eq!(versions, (1..=54).collect::<Vec<_>>());
    let index: Value = sqlx::query_scalar("SELECT jsonb_build_object('valid',i.indisvalid,'ready',i.indisready,'unique',i.indisunique,'keys',i.indnkeyatts,
        'attributes',(SELECT jsonb_agg(a.attname ORDER BY k.ord) FROM unnest(i.indkey) WITH ORDINALITY k(attnum,ord) JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum),
        'noPredicate',i.indpred IS NULL,'noExpression',i.indexprs IS NULL)
        FROM pg_index i WHERE i.indexrelid='fvoci.task_timer_audit_user_id_idx'::regclass AND i.indrelid='fvoci.task_timer_audit'::regclass")
        .fetch_one(&fx.admin).await.unwrap();
    assert_eq!(
        index,
        json!({"valid":true,"ready":true,"unique":false,"keys":1,"attributes":["user_id"],"noPredicate":true,"noExpression":true})
    );
    let after_migration_metadata: Value = sqlx::query_scalar(migration_metadata_sql)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    // schema_migrations stores versions/timestamps, not SQL checksums. Pair the
    // actual database rows with source bytes from this frozen runner input.
    use sha2::{Digest, Sha256};
    let mut source_checksums: Vec<Value> = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
    )
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
    .map(|path| {
        let file = path.file_name().unwrap().to_str().unwrap();
        let version: i32 = file.split('_').next().unwrap().parse().unwrap();
        json!({"version":version,"file":file,"sha256":format!("{:x}",Sha256::digest(std::fs::read(&path).unwrap()))})
    })
    .collect();
    source_checksums.sort_by_key(|row| row["version"].as_i64().unwrap());
    assert_eq!(source_checksums.len(), 54);
    println!(
        "W8-CURRENT-054-FULL-SCHEMA {}",
        json!({"beforeSecurity":before_security,"afterSecurity":after_security,
            "beforeDataFingerprints":before_data,"afterDataFingerprints":after_data,
            "beforeMigrationRows":before_migration_metadata,"afterMigrationRows":after_migration_metadata,
            "frozenSourceChecksums":source_checksums,"databaseStoresChecksums":false})
    );
    println!(
        "W8-CURRENT-054-CATALOG {}",
        json!({"index":index,"role":role,"versions":versions,"securityUnchanged":true,"historyUnchanged":true})
    );
    measure("actor 2048, other 0").await;
    fill(fx.user_id, 20_000 - 2048).await;
    measure("actor 20000, other 0").await;
    fill(other.user_id, 20_000).await;
    measure("actor 20000, other 20000").await;

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_carries_the_collection_of_a_renamed_project() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "RNM", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"이름 바뀐 프로젝트의 태스크"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    // An ordinary rename: the 028 trigger-made task collection keeps the name
    // the project had when it was created.
    let (status, patched) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}"),
        Some(json!({"name":"바뀐 이름 🙂"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {patched}");
    let source: (Uuid, String, i32) =
        sqlx::query_as("SELECT id, name, version FROM fvoci.collections WHERE project_id=$1")
            .bind(project_id)
            .fetch_one(&fx.admin)
            .await
            .unwrap();
    assert_ne!(
        source.1, "바뀐 이름 🙂",
        "the collection keeps its original name"
    );
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("a renamed project is capturable");
    captured.archive.validate().expect("and valid");
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("and restorable");
    let restored: (Uuid, String, i32) = sqlx::query_as(
        "SELECT id, name, version FROM fvoci.collections WHERE workspace_id=$1 AND project_id=$2",
    )
    .bind(destination)
    .bind(project_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(restored, source);

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_restores_person_made_collections_for_a_fresh_client() {
    use fvoci_server::db::native_archive::{capture, publish};
    use fvoci_server::native_archive::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let call = |app: axum::Router,
                method: &'static str,
                path: String,
                body: Option<Value>,
                cookie: String| async move {
        let (status, reply) = json_request(app, method, &path, body, Some(&cookie)).await;
        assert!(status.is_success(), "{method} {path}: {status} {reply}");
        reply
    };
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "COL", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let w = format!("/api/v1/workspaces/{ws}");
    let task = call(
        fx.app.clone(),
        "POST",
        format!("{w}/projects/{project_id}/tasks"),
        Some(json!({"title":"필드 대상 🧪"})),
        fx.cookie.clone(),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_owned();
    // The task is assigned to its author (the assignee filter below matches).
    call(
        fx.app.clone(),
        "PATCH",
        format!("{w}/tasks/{task_id}"),
        Some(json!({"assigneeIds":[fx.user_id]})),
        fx.cookie.clone(),
    )
    .await;
    // An ordinary project rename (the collection keeps its first name).
    call(
        fx.app.clone(),
        "PATCH",
        format!("{w}/projects/{project_id}"),
        Some(json!({"name":"바뀐 컬렉션 프로젝트"})),
        fx.cookie.clone(),
    )
    .await;
    let collection = call(
        fx.app.clone(),
        "GET",
        format!("{w}/projects/{project_id}/collection"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let cid = collection["id"].as_str().unwrap().to_owned();
    let field =
        |name: &str, kind: &str, options: Value| json!({"name":name,"type":kind,"options":options});
    let mut fields = std::collections::BTreeMap::new();
    for (name, kind, options) in [
        ("단계", "select", json!(["준비", "진행", "완료"])),
        ("태그", "multi_select", json!(["가", "나"])),
        ("점검", "checkboxes", json!(["하나", "둘"])),
        ("라벨", "labels", json!(["빨강"])),
        ("점수", "number", json!([])),
        ("마감일", "date", json!([])),
        ("시각", "datetime", json!([])),
        ("확인", "checkbox", json!([])),
        ("요약", "text", json!([])),
        ("메모", "paragraph", json!([])),
        ("담당", "user", json!([])),
        ("폐기 필드", "text", json!([])),
    ] {
        let created = call(
            fx.app.clone(),
            "POST",
            format!("{w}/collections/{cid}/fields"),
            Some(field(name, kind, options)),
            fx.cookie.clone(),
        )
        .await;
        fields.insert(name, created);
    }
    // Field patch: rename, relabel, add an option.
    let stage = fields["단계"].clone();
    let opts = stage["options"].as_array().unwrap().clone();
    call(
        fx.app.clone(),
        "PATCH",
        format!(
            "{w}/collections/{cid}/fields/{}",
            stage["id"].as_str().unwrap()
        ),
        Some(
            json!({"expectedVersion":stage["version"],"name":"진행 단계","options":[
            {"id":opts[0]["id"],"label":"준비"},{"id":opts[1]["id"],"label":"진행 중"},
            {"id":opts[2]["id"],"label":"완료","deleted":false},{"label":"보류"}]}),
        ),
        fx.cookie.clone(),
    )
    .await;
    let item = call(
        fx.app.clone(),
        "GET",
        format!("{w}/tasks/{task_id}/collection-item"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let item_id = item["item"]["id"].as_str().unwrap().to_owned();
    let mut version = item["item"]["version"].as_i64().unwrap();
    let put = |field: &Value, value: Value, version: i64| json!({"fieldId":field["id"],"expectedVersion":version,"expectedFieldVersion":field["version"],"value":value});
    let refreshed = |name: &str, list: &Value| {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()
            .clone()
    };
    let listed = call(
        fx.app.clone(),
        "GET",
        format!("{w}/collections/{cid}/fields"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let stage = refreshed("진행 단계", &listed);
    let archived_option = stage["options"][2]["id"].clone();
    // A text value whose literal text contains the source person and
    // workspace ids: plain text, never an identity reference.
    let literal = format!("요약 🧪 {} {}", fx.user_id, ws);
    for (name, value) in [
        ("진행 단계", json!({"options":[archived_option]})),
        (
            "태그",
            json!({"options":[fields["태그"]["options"][0]["id"], fields["태그"]["options"][1]["id"]]}),
        ),
        (
            "점검",
            json!({"options":[fields["점검"]["options"][1]["id"]]}),
        ),
        (
            "라벨",
            json!({"options":[fields["라벨"]["options"][0]["id"]]}),
        ),
        ("점수", json!({"number":2.75})),
        ("마감일", json!({"date":"2026-10-31"})),
        ("시각", json!({"datetime":"2026-10-03T09:30:00.000Z"})),
        ("확인", json!({"checkbox":true})),
        ("요약", json!({"text":literal})),
        ("메모", json!({"text":"여러 줄\n메모"})),
        ("담당", json!({"users":[fx.user_id]})),
        ("폐기 필드", json!({"text":"남는 값"})),
    ] {
        let f = refreshed(name, &listed);
        let reply = call(
            fx.app.clone(),
            "PUT",
            format!("{w}/collections/{cid}/items/{item_id}/values"),
            Some(put(&f, value, version)),
            fx.cookie.clone(),
        )
        .await;
        version = reply["version"].as_i64().unwrap();
    }
    // Archive the chosen option afterwards (it stays chosen), soft-delete a field.
    let listed = call(
        fx.app.clone(),
        "GET",
        format!("{w}/collections/{cid}/fields"),
        None,
        fx.cookie.clone(),
    )
    .await;
    let stage = refreshed("진행 단계", &listed);
    let options: Vec<Value> = stage["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| json!({"id":o["id"],"label":o["label"],"deleted":o["id"] == archived_option}))
        .collect();
    call(
        fx.app.clone(),
        "PATCH",
        format!(
            "{w}/collections/{cid}/fields/{}",
            stage["id"].as_str().unwrap()
        ),
        Some(json!({"expectedVersion":stage["version"],"options":options})),
        fx.cookie.clone(),
    )
    .await;
    let gone = refreshed("폐기 필드", &listed);
    call(
        fx.app.clone(),
        "PATCH",
        format!(
            "{w}/collections/{cid}/fields/{}",
            gone["id"].as_str().unwrap()
        ),
        Some(json!({"expectedVersion":gone["version"],"deleted":true})),
        fx.cookie.clone(),
    )
    .await;
    // Views: a shared board grouped by the select field; a private calendar
    // whose query names the source person (assignee, and a people value in
    // the uppercase spelling the writer accepts), an option value, a text
    // value equal to the source person's id (plain text) and a field sort.
    let due = refreshed("마감일", &listed);
    let owner_field = refreshed("담당", &listed);
    let tag = refreshed("태그", &listed);
    let summary = refreshed("요약", &listed);
    let board = call(fx.app.clone(), "POST", format!("{w}/collections/{cid}/views"), Some(json!({"name":"단계 보드","type":"board","visibility":"shared",
        "config":{"query":{"filters":{"openOnly":true},"sort":[{"field":"due","direction":"asc"}]},"groupBy":stage["id"],"dateBy":null}})), fx.cookie.clone()).await;
    let calendar_query = json!({"filters":{"assigneeId":fx.user_id.to_string(),"custom":[
        {"fieldId":tag["id"],"operator":"equals","value":tag["options"][0]["id"]},
        {"fieldId":owner_field["id"],"operator":"equals","value":fx.user_id.to_string().to_uppercase()},
        {"fieldId":summary["id"],"operator":"equals","value":literal}]},
        "sort":[{"field":refreshed("점수", &listed)["id"],"direction":"desc"}]});
    let calendar = call(
        fx.app.clone(),
        "POST",
        format!("{w}/collections/{cid}/views"),
        Some(
            json!({"name":"내 달력","type":"calendar","visibility":"private",
        "config":{"query":calendar_query,"groupBy":null,"dateBy":due["id"]}}),
        ),
        fx.cookie.clone(),
    )
    .await;
    // A project document collection with the project's root document.
    let root: Uuid = sqlx::query_scalar("SELECT root_document_id FROM fvoci.projects WHERE id=$1")
        .bind(project_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    let docs = call(
        fx.app.clone(),
        "POST",
        format!("{w}/collections"),
        Some(json!({"name":"자료","kind":"document","projectId":project_id})),
        fx.cookie.clone(),
    )
    .await;
    let did = docs["id"].as_str().unwrap().to_owned();
    let doc_field = call(
        fx.app.clone(),
        "POST",
        format!("{w}/collections/{did}/fields"),
        Some(field("출처", "text", json!([]))),
        fx.cookie.clone(),
    )
    .await;
    let doc_item = call(
        fx.app.clone(),
        "POST",
        format!("{w}/collections/{did}/items"),
        Some(json!({"documentId":root})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PUT",
        format!(
            "{w}/collections/{did}/items/{}/values",
            doc_item["id"].as_str().unwrap()
        ),
        Some(put(
            &doc_field,
            json!({"text":"원문"}),
            doc_item["version"].as_i64().unwrap(),
        )),
        fx.cookie.clone(),
    )
    .await;

    // Reads through the ordinary routes: fields, default-query items, views,
    // and the saved calendar query itself.
    let default_query =
        json!({"config":{"query":{"filters":{},"sort":[]},"groupBy":null,"dateBy":null}});
    let read = |app: axum::Router, w: String, cookie: String, saved: Value| {
        let (cid, did, default_query) = (cid.clone(), did.clone(), default_query.clone());
        async move {
            let mut out = Vec::new();
            for c in [&cid, &did] {
                out.push(
                    call(
                        app.clone(),
                        "GET",
                        format!("{w}/collections/{c}/fields"),
                        None,
                        cookie.clone(),
                    )
                    .await,
                );
                out.push(
                    call(
                        app.clone(),
                        "POST",
                        format!("{w}/collections/{c}/query"),
                        Some(default_query.clone()),
                        cookie.clone(),
                    )
                    .await["items"]
                        .clone(),
                );
                out.push(
                    call(
                        app.clone(),
                        "GET",
                        format!("{w}/collections/{c}/views"),
                        None,
                        cookie.clone(),
                    )
                    .await,
                );
            }
            out.push(
                call(
                    app.clone(),
                    "POST",
                    format!("{w}/collections/{cid}/query"),
                    Some(json!({"config":{"query":saved["query"],"groupBy":null,"dateBy":null}})),
                    cookie.clone(),
                )
                .await["items"]
                    .clone(),
            );
            out
        }
    };
    let source_reads = read(
        fx.app.clone(),
        w.clone(),
        fx.cookie.clone(),
        calendar["config"].clone(),
    )
    .await;
    assert_eq!(
        source_reads[6].as_array().unwrap().len(),
        1,
        "the saved calendar query matches the task"
    );

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("person-made collections are captured");
    let g = &captured.archive.graph;
    assert_eq!(
        (
            g.collections.len(),
            g.collection_fields.len(),
            g.collection_views.len()
        ),
        (2, 13, 2)
    );
    assert!(g.collection_fields.iter().any(|f| f.deleted_at.is_some()));
    assert!(g.collection_options.iter().any(|o| o.deleted_at.is_some()));
    assert_eq!(g.collection_people.len(), 1);
    captured
        .archive
        .validate()
        .expect("the archive policy accepts the collections");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("collections restore");
    let dw = format!("/api/v1/workspaces/{destination}");
    // Independent oracle: the source reads with only the known typed person
    // references changed (view owner, the people value of the user field,
    // the assignee and the people custom value of the saved query); every
    // text literal (including the one containing the ids) stays as stored.
    let (src, dst_id) = (fx.user_id, dst.user_id);
    let people_field = owner_field["id"].clone();
    let map_items = |items: &Value| {
        let mut items = items.clone();
        for item in items.as_array_mut().unwrap() {
            if let Some(users) = item["values"]
                .get_mut(people_field.as_str().unwrap())
                .and_then(|v| v.get_mut("users"))
            {
                for user in users.as_array_mut().unwrap() {
                    if *user == json!(src) {
                        *user = json!(dst_id);
                    }
                }
            }
        }
        items
    };
    let map_views = |views: &Value| {
        let mut views = views.clone();
        for view in views["items"].as_array_mut().unwrap() {
            if view["ownerId"] == json!(src) {
                view["ownerId"] = json!(dst_id);
            }
            let filters = &mut view["config"]["query"]["filters"];
            if filters.get("assigneeId") == Some(&json!(src.to_string())) {
                filters["assigneeId"] = json!(dst_id.to_string());
            }
            for custom in filters
                .get_mut("custom")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if custom["fieldId"] == people_field
                    && custom["value"] == json!(src.to_string().to_uppercase())
                {
                    custom["value"] = json!(dst_id.to_string());
                }
            }
        }
        views
    };
    let expected = vec![
        source_reads[0].clone(),
        map_items(&source_reads[1]),
        map_views(&source_reads[2]),
        source_reads[3].clone(),
        map_items(&source_reads[4]),
        map_views(&source_reads[5]),
        map_items(&source_reads[6]),
    ];
    let restored_calendar =
        map_views(&json!({"items":[calendar.clone()]}))["items"][0]["config"].clone();
    let restored_reads = read(
        dst.app.clone(),
        dw.clone(),
        dst.cookie.clone(),
        restored_calendar,
    )
    .await;
    assert_eq!(restored_reads, expected);
    let summary_value = &restored_reads[1][0]["values"][summary["id"].as_str().unwrap()];
    assert_eq!(
        summary_value,
        &json!({"text":literal}),
        "the literal text keeps the source ids"
    );
    // The writers continue from the restored versions.
    let listed = call(
        dst.app.clone(),
        "GET",
        format!("{dw}/collections/{cid}/fields"),
        None,
        dst.cookie.clone(),
    )
    .await;
    let score = refreshed("점수", &listed);
    let reply = call(
        dst.app.clone(),
        "PUT",
        format!("{dw}/collections/{cid}/items/{item_id}/values"),
        Some(put(&score, json!({"number":3}), version)),
        dst.cookie.clone(),
    )
    .await;
    assert_eq!(reply["version"].as_i64(), Some(version + 1));
    let restored_board = call(
        dst.app.clone(),
        "GET",
        format!("{dw}/collections/{cid}/views"),
        None,
        dst.cookie.clone(),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == board["id"])
        .unwrap()
        .clone();
    call(dst.app.clone(), "PATCH", format!("{dw}/collections/{cid}/views/{}", board["id"].as_str().unwrap()),
        Some(json!({"name":"복원된 보드","type":"board","visibility":"shared","config":restored_board["config"],"expectedVersion":restored_board["version"]})), dst.cookie.clone()).await;

    // Another person in a user field and another person's private view are
    // typed refusals of this single-author slice (the view row is a labeled
    // admin fixture writer: the other member cannot open the private project).
    let other =
        project_harness::add_workspace_user(&fx.admin, ws, "member", "collection-other").await;
    let item_now = call(
        fx.app.clone(),
        "GET",
        format!("{w}/tasks/{task_id}/collection-item"),
        None,
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PUT",
        format!("{w}/collections/{cid}/items/{item_id}/values"),
        Some(put(
            &owner_field,
            json!({"users":[other.user_id]}),
            item_now["item"]["version"].as_i64().unwrap(),
        )),
        fx.cookie.clone(),
    )
    .await;
    let refused = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .unwrap()
        .archive
        .validate();
    assert!(
        matches!(&refused, Err(ArchiveError::Unsupported(m)) if m == "collection people"),
        "{refused:?}"
    );
    let item_now = call(
        fx.app.clone(),
        "GET",
        format!("{w}/tasks/{task_id}/collection-item"),
        None,
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PUT",
        format!("{w}/collections/{cid}/items/{item_id}/values"),
        Some(put(
            &owner_field,
            json!({"users":[fx.user_id]}),
            item_now["item"]["version"].as_i64().unwrap(),
        )),
        fx.cookie.clone(),
    )
    .await;
    sqlx::query("INSERT INTO fvoci.collection_views (id, workspace_id, collection_id, owner_id, visibility, name, type, config) VALUES ($1,$2,$3,$4,'private','남의 보기','table','{\"query\":{\"filters\":{},\"sort\":[]},\"groupBy\":null,\"dateBy\":null}')")
        .bind(Uuid::now_v7()).bind(ws).bind(Uuid::parse_str(&cid).unwrap()).bind(other.user_id)
        .execute(&fx.admin).await.unwrap();
    let refused = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .unwrap()
        .archive
        .validate();
    assert!(
        matches!(&refused, Err(ArchiveError::Unsupported(m)) if m == "collection views"),
        "{refused:?}"
    );

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_carries_a_long_project_named_task_collection() {
    use fvoci_server::db::native_archive::{capture, publish};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    // The project writer accepts 200 characters and the 028 trigger copies the
    // project's name into its task collection: 101 characters exceed the
    // collection writer's 100 UTF-16 limit but are a valid current name.
    let long = "가".repeat(101);
    let (status, project) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects"),
        Some(json!({"key":"LNG","name":long,"visibility":"private"})),
        Some(&fx.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        fx.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title":"긴 이름"})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {task}");
    let (status, renamed) = json_request(
        fx.app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}"),
        Some(json!({"name":"나".repeat(150)})),
        Some(&fx.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {renamed}");
    let name: String = sqlx::query_scalar("SELECT name FROM fvoci.collections WHERE project_id=$1")
        .bind(project_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    assert_eq!(name, long);
    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("a long project name is capturable");
    captured.archive.validate().expect("and valid");
    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("and restorable");
    let restored: String = sqlx::query_scalar(
        "SELECT name FROM fvoci.collections WHERE workspace_id=$1 AND project_id=$2",
    )
    .bind(destination)
    .bind(project_id)
    .fetch_one(&dst.admin)
    .await
    .unwrap();
    assert_eq!(restored, long);

    let storages = [fx.storage_root(), dst.storage_root()];
    fx.pool.close().await;
    fx.admin.close().await;
    dst.pool.close().await;
    dst.admin.close().await;
    harness.cleanup().await;
    destination_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

#[tokio::test]
async fn native_archive_capture_graph_budget_is_cumulative_and_row_bounded() {
    use fvoci_server::db::native_archive::{capture, NativeDbError};
    use fvoci_server::native_archive::ArchiveError;
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let project_with_task = |key: &'static str| {
        let (app, cookie) = (fx.app.clone(), fx.cookie.clone());
        async move {
            let project =
                project_harness::create_project(app.clone(), &cookie, ws, key, "private").await;
            let id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
            let (status, task) = json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/projects/{id}/tasks"),
                Some(json!({"title":"예산"})),
                Some(&cookie),
            )
            .await;
            assert!(status.is_success(), "{status} {task}");
            id
        }
    };
    let collection_of = |project: Uuid| {
        let admin = fx.admin.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM fvoci.collections WHERE project_id=$1 AND kind='task'",
            )
            .bind(project)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    };
    let limited = |r: &Result<_, NativeDbError>| {
        matches!(r, Err(NativeDbError::Archive(ArchiveError::Limit)))
    };
    // Cumulative: the task array and the collection value array are each
    // below the 16 MiB graph budget, together above it (labeled admin fixture
    // writer: sizes no ordinary writer reaches in one request).
    let big = project_with_task("BIG").await;
    let collection = collection_of(big).await;
    sqlx::query("UPDATE fvoci.tasks SET text = repeat('가', 3000000) WHERE project_id=$1")
        .bind(big)
        .execute(&fx.admin)
        .await
        .unwrap();
    let field = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.collection_fields (id, workspace_id, collection_id, key, name, type, sort_key) VALUES ($1,$2,$3,'memo','메모','paragraph','000')")
        .bind(field).bind(ws).bind(collection).execute(&fx.admin).await.unwrap();
    sqlx::query("INSERT INTO fvoci.collection_values (workspace_id, collection_id, item_id, field_id, field_type, value_text)
                 SELECT $1, $2, i.id, $3, 'paragraph', repeat('나', 2800000) FROM fvoci.collection_items i WHERE i.collection_id=$2")
        .bind(ws).bind(collection).bind(field).execute(&fx.admin).await.unwrap();
    let (task_bytes, value_bytes): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT sum(octet_length((to_jsonb(t)-'workspace_id')::text)) FROM fvoci.tasks t WHERE t.project_id=$1)::bigint,
                (SELECT sum(octet_length((to_jsonb(v)-'workspace_id')::text)) FROM fvoci.collection_values v WHERE v.collection_id=$2)::bigint",
    )
    .bind(big)
    .bind(collection)
    .fetch_one(&fx.admin)
    .await
    .unwrap();
    let budget = 16 * 1024 * 1024;
    assert!(
        task_bytes < budget && value_bytes < budget && task_bytes + value_bytes > budget,
        "{task_bytes} {value_bytes}"
    );
    let result = capture(&fx.pool, ws, fx.user_id, session, &project_only(big)).await;
    assert!(limited(&result), "cumulative: {:?}", result.err());
    // Rows: a 10001st row of one array is a Limit, never a truncation.
    let many = project_with_task("ROW").await;
    let collection = collection_of(many).await;
    let field = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.collection_fields (id, workspace_id, collection_id, key, name, type, sort_key) VALUES ($1,$2,$3,'stage','단계','select','000')")
        .bind(field).bind(ws).bind(collection).execute(&fx.admin).await.unwrap();
    sqlx::query("INSERT INTO fvoci.collection_options (id, workspace_id, collection_id, field_id, key, label, sort_key)
                 SELECT gen_random_uuid(), $1, $2, $3, 'o_' || n, 'o' || n, lpad(n::text, 5, '0') FROM generate_series(1, 10001) n")
        .bind(ws).bind(collection).bind(field).execute(&fx.admin).await.unwrap();
    let result = capture(&fx.pool, ws, fx.user_id, session, &project_only(many)).await;
    assert!(limited(&result), "rows: {:?}", result.err());

    let storage = fx.storage_root();
    fx.pool.close().await;
    fx.admin.close().await;
    harness.cleanup().await;
    std::fs::remove_dir_all(storage).unwrap();
}

#[tokio::test]
async fn native_archive_restores_document_tags_for_a_fresh_client() {
    use fvoci_server::db::native_archive::{capture, publish, NativeDbError};
    let harness = TestDb::bootstrap().await;
    let fx = fixture(&harness).await;
    let session = project_harness::session_id_for_user(&fx.admin, fx.user_id).await;
    let ws = fx.workspace_id;
    let call = |app: axum::Router,
                method: &'static str,
                path: String,
                body: Option<Value>,
                cookie: String| async move {
        let (status, reply) = json_request(app, method, &path, body, Some(&cookie)).await;
        assert!(status.is_success(), "{method} {path}: {status} {reply}");
        reply
    };
    let project =
        project_harness::create_project(fx.app.clone(), &fx.cookie, ws, "TAG", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let w = format!("/api/v1/workspaces/{ws}");
    let root: Uuid = sqlx::query_scalar("SELECT root_document_id FROM fvoci.projects WHERE id=$1")
        .bind(project_id)
        .fetch_one(&fx.admin)
        .await
        .unwrap();
    let child = call(
        fx.app.clone(),
        "POST",
        format!("{w}/projects/{project_id}/documents"),
        Some(json!({"parentId":root,"title":"하위 문서 🧪"})),
        fx.cookie.clone(),
    )
    .await;
    let child_id = child["id"].as_str().unwrap().to_owned();
    // Tags through the ordinary writers: one renamed and one recolored by the
    // owner (an admin), one never assigned.
    let red = call(
        fx.app.clone(),
        "POST",
        format!("{w}/document-tags"),
        Some(json!({"name":"검토","color":"red"})),
        fx.cookie.clone(),
    )
    .await;
    let blue = call(
        fx.app.clone(),
        "POST",
        format!("{w}/document-tags"),
        Some(json!({"name":"Ref 참고 🧪"})),
        fx.cookie.clone(),
    )
    .await;
    let unused = call(
        fx.app.clone(),
        "POST",
        format!("{w}/document-tags"),
        Some(json!({"name":"안 씀"})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PATCH",
        format!("{w}/document-tags/{}", red["id"].as_str().unwrap()),
        Some(json!({"name":"검토 완료"})),
        fx.cookie.clone(),
    )
    .await;
    call(
        fx.app.clone(),
        "PATCH",
        format!("{w}/document-tags/{}", blue["id"].as_str().unwrap()),
        Some(json!({"color":"blue"})),
        fx.cookie.clone(),
    )
    .await;
    let docs_base = format!("{w}/projects/{project_id}/documents");
    for (doc, tag) in [
        (root.to_string(), &red),
        (root.to_string(), &blue),
        (child_id.clone(), &blue),
    ] {
        call(
            fx.app.clone(),
            "POST",
            format!("{docs_base}/{doc}/tags"),
            Some(json!({"tagId":tag["id"]})),
            fx.cookie.clone(),
        )
        .await;
    }
    let read = |app: axum::Router, w: String, cookie: String| {
        let (child_id, red_id) = (child_id.clone(), red["id"].as_str().unwrap().to_owned());
        async move {
            let docs_base = format!("{w}/projects/{project_id}/documents");
            let mut out = Vec::new();
            for doc in [root.to_string(), child_id] {
                out.push(
                    call(
                        app.clone(),
                        "GET",
                        format!("{docs_base}/{doc}/tags"),
                        None,
                        cookie.clone(),
                    )
                    .await,
                );
            }
            let tree = call(
                app.clone(),
                "GET",
                format!("{docs_base}?tag={red_id}"),
                None,
                cookie.clone(),
            )
            .await;
            let mut ids: Vec<String> = tree["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["id"].as_str().unwrap().to_owned())
                .collect();
            ids.sort();
            out.push(json!(ids));
            let pool = call(
                app.clone(),
                "GET",
                format!("{w}/document-tags"),
                None,
                cookie.clone(),
            )
            .await;
            let mut names: Vec<(String, String, String)> = pool["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    (
                        t["id"].as_str().unwrap().to_owned(),
                        t["name"].as_str().unwrap().to_owned(),
                        t["color"].as_str().unwrap().to_owned(),
                    )
                })
                .collect();
            names.sort();
            out.push(json!(names));
            out
        }
    };
    let source_reads = read(fx.app.clone(), w.clone(), fx.cookie.clone()).await;

    let captured = capture(&fx.pool, ws, fx.user_id, session, &project_only(project_id))
        .await
        .expect("document tags are capturable");
    // Read through the serialized graph so this test also compiles on the
    // pre-change source (its RED is a capture refusal).
    let graph = serde_json::to_value(&captured.archive.graph).unwrap();
    let mut tag_ids: Vec<String> = graph["document_tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    tag_ids.sort();
    let mut expected = vec![
        red["id"].as_str().unwrap().to_owned(),
        blue["id"].as_str().unwrap().to_owned(),
    ];
    expected.sort();
    assert_eq!(
        tag_ids, expected,
        "the unassigned workspace tag stays behind"
    );
    assert!(!tag_ids.contains(&unused["id"].as_str().unwrap().to_owned()));
    assert_eq!(
        graph["document_tag_assignments"].as_array().unwrap().len(),
        3
    );
    captured.archive.validate().expect("and valid");

    let destination_db = TestDb::bootstrap().await;
    let dst = fixture(&destination_db).await;
    let (destination, _, claim) = claimed_restore(&dst, dst.user_id, &dst.cookie).await;
    publish(
        &dst.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &dst.settings.quota,
    )
    .await
    .expect("document tags restore");
    let dw = format!("/api/v1/workspaces/{destination}");
    // The destination client reads the same tags per document, the same tree
    // filter result, and a pool of exactly the restored tags.
    let restored_reads = read(dst.app.clone(), dw.clone(), dst.cookie.clone()).await;
    let source_pool: Vec<Value> = source_reads[3]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t[0] != json!(unused["id"]))
        .cloned()
        .collect();
    // Tags are workspace rows: their workspaceId is the restore workspace;
    // every other field (ids, names, colors, timestamps) is unchanged.
    let mut expected_reads = source_reads[..3].to_vec();
    for document in expected_reads.iter_mut().take(2) {
        for tag in document["items"].as_array_mut().unwrap() {
            assert_eq!(tag["workspaceId"], json!(ws));
            tag["workspaceId"] = json!(destination);
        }
    }
    assert_eq!(&restored_reads[..3], &expected_reads[..]);
    assert_eq!(restored_reads[3], json!(source_pool));
    // The writers continue on the restored tags.
    let ddocs = format!("{dw}/projects/{project_id}/documents");
    call(
        dst.app.clone(),
        "POST",
        format!("{ddocs}/{child_id}/tags"),
        Some(json!({"tagId":red["id"]})),
        dst.cookie.clone(),
    )
    .await;
    call(
        dst.app.clone(),
        "DELETE",
        format!("{ddocs}/{root}/tags/{}", blue["id"].as_str().unwrap()),
        None,
        dst.cookie.clone(),
    )
    .await;
    call(
        dst.app.clone(),
        "PATCH",
        format!("{dw}/document-tags/{}", blue["id"].as_str().unwrap()),
        Some(json!({"name":"참고 끝"})),
        dst.cookie.clone(),
    )
    .await;
    // ... and their effects are read back, not only their 2xx.
    let after_writes = read(dst.app.clone(), dw.clone(), dst.cookie.clone()).await;
    let tag_names = |tags: &Value| {
        let mut names: Vec<String> = tags["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(tag_names(&after_writes[0]), vec!["검토 완료".to_owned()]);
    assert_eq!(
        tag_names(&after_writes[1]),
        vec!["검토 완료".to_owned(), "참고 끝".to_owned()]
    );

    // A destination personal workspace already holding a tag of the same name
    // in another case refuses the whole restore, with no effects.
    let busy_db = TestDb::bootstrap().await;
    let busy = fixture(&busy_db).await;
    let (status, personal) = json_request(
        busy.app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&busy.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {personal}");
    let personal_id = personal["id"].as_str().unwrap().to_owned();
    call(
        busy.app.clone(),
        "POST",
        format!("/api/v1/workspaces/{personal_id}/document-tags"),
        Some(json!({"name":"REF 참고 🧪"})),
        busy.cookie.clone(),
    )
    .await;
    let (target, _, claim) = claimed_restore(&busy, busy.user_id, &busy.cookie).await;
    assert_eq!(target.to_string(), personal_id);
    // Bounded before/after observation of the rejected publish: every
    // application table's row count and row digest (no payload printed), the
    // existing tag's exact row, and the storage inventory.
    let existing_tag = |admin: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(t) FROM fvoci.document_tags t WHERE lower(name)=lower('REF 참고 🧪')",
        )
        .fetch_one(&admin)
        .await
        .unwrap()
    };
    let tables_before = application_table_digests(&busy.admin).await;
    let tag_before = existing_tag(busy.admin.clone()).await;
    let storage_before = storage_inventory(&busy.storage_root());
    let refused = publish(
        &busy.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &busy.settings.quota,
    )
    .await;
    // Direct publish surfaces the database's unique violation (the HTTP
    // route maps it to 409), the same strict oracle as the late-collision
    // test.
    let unique_violation = |result: &Result<_, NativeDbError>| matches!(result, Err(NativeDbError::Sql(error)) if error.as_database_error().is_some_and(|e| e.is_unique_violation()));
    assert!(unique_violation(&refused), "{refused:?}");
    let effects: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM fvoci.projects WHERE workspace_id=$1)+(SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1)+(SELECT count(*) FROM fvoci.document_tag_assignments WHERE workspace_id=$1)+(SELECT count(*) FROM fvoci.document_tags WHERE workspace_id=$1)")
        .bind(target)
        .fetch_one(&busy.admin)
        .await
        .unwrap();
    assert_eq!(effects, 1, "only the pre-existing tag remains");
    assert_eq!(application_table_digests(&busy.admin).await, tables_before);
    assert_eq!(existing_tag(busy.admin.clone()).await, tag_before);
    assert_eq!(tag_before["color"], json!("gray"));
    assert_eq!(storage_inventory(&busy.storage_root()), storage_before);

    // A destination that already holds a tag with an archived tag's id (a
    // different name; labeled admin fixture writer) is the same refusal.
    let taken_db = TestDb::bootstrap().await;
    let taken = fixture(&taken_db).await;
    let (target, _, claim) = claimed_restore(&taken, taken.user_id, &taken.cookie).await;
    sqlx::query("INSERT INTO fvoci.document_tags (id, workspace_id, name, color) VALUES ($1::text::uuid, $2, '다른 이름', 'green')")
        .bind(red["id"].as_str().unwrap())
        .bind(target)
        .execute(&taken.admin)
        .await
        .unwrap();
    let tables_before = application_table_digests(&taken.admin).await;
    let refused = publish(
        &taken.pool,
        &claim,
        &captured.archive,
        &std::collections::BTreeMap::new(),
        &taken.settings.quota,
    )
    .await;
    assert!(unique_violation(&refused), "{refused:?}");
    assert_eq!(application_table_digests(&taken.admin).await, tables_before);

    let storages = [
        fx.storage_root(),
        dst.storage_root(),
        busy.storage_root(),
        taken.storage_root(),
    ];
    for f in [&fx, &dst, &busy, &taken] {
        f.pool.close().await;
        f.admin.close().await;
    }
    harness.cleanup().await;
    destination_db.cleanup().await;
    busy_db.cleanup().await;
    taken_db.cleanup().await;
    for storage in storages {
        std::fs::remove_dir_all(storage).unwrap();
    }
}

/// Row count and an order-independent digest of every fvoci table (admin
/// read; only digests leave the database). Test-local observation.
async fn application_table_digests(
    admin: &sqlx::PgPool,
) -> std::collections::BTreeMap<String, (i64, String)> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema='fvoci' AND table_type='BASE TABLE' ORDER BY 1",
    )
    .fetch_all(admin)
    .await
    .unwrap();
    let mut out = std::collections::BTreeMap::new();
    for table in tables {
        let row: (i64, String) = sqlx::query_as(&format!(
            "SELECT count(*)::bigint, coalesce(md5(string_agg(md5(t::text), '' ORDER BY md5(t::text))), '') FROM fvoci.\"{table}\" t"
        ))
        .fetch_one(admin)
        .await
        .unwrap();
        out.insert(table, row);
    }
    out
}

/// Relative path and size of every stored object under a storage root. Any
/// directory or metadata read failure fails the test (an unreadable store is
/// never an empty one).
fn storage_inventory(root: &std::path::Path) -> Vec<(String, u64)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, u64)>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("entry in {}: {e}", dir.display()));
            let path = entry.path();
            let metadata = entry
                .metadata()
                .unwrap_or_else(|e| panic!("metadata {}: {e}", path.display()));
            if metadata.is_dir() {
                walk(&path, root, out);
            } else {
                out.push((
                    path.strip_prefix(root).unwrap().display().to_string(),
                    metadata.len(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}
