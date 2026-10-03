//! W2 x W6 regressions for owner-private Zotero rows (migration 051, copied
//! byte-identical from W6 f6297f89 under the root's schema-only adoption).
//!
//! zotero_links cascade and zotero_references SET NULL on a document/task
//! delete, so a MOVE (delete under the source, insert under the destination)
//! would silently destroy or detach the owner's private Zotero metadata.
//! MOVE must refuse with dependent_graph before any effect; COPY must carry
//! none of these private rows (credentials never).

use super::*;

/// Every Zotero table of a workspace as committed rows. Read through the
/// fixture admin pool: the tables are FORCE RLS tenant AND owner scoped, and
/// the assertion is that no owner's row changed, not only the actor's.
async fn zotero_graph(run: &TestRun, ws: Uuid) -> Value {
    let admin = admin_pool(&run.harness).await;
    let mut graph = serde_json::Map::new();
    for table in [
        "zotero_connectors",
        "zotero_credentials",
        "zotero_references",
        "zotero_collections",
        "zotero_memberships",
        "zotero_links",
    ] {
        let sql = format!("SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY to_jsonb(r)::text),'[]'::jsonb) FROM fvoci.{table} r WHERE workspace_id=$1");
        let rows: Value = sqlx::query_scalar(&sql)
            .bind(ws)
            .fetch_one(&admin)
            .await
            .unwrap();
        graph.insert(table.into(), rows);
    }
    admin.close().await;
    Value::Object(graph)
}

#[derive(Clone, Copy, Debug)]
enum ZoteroEdge {
    ReferenceOnDocument,
    LinkOnDocument,
    LinkOnTask,
}

/// Fixture-only owner-private rows in the personal source workspace, owned by
/// the actor (the only member of a personal workspace). The sealed key is a
/// fixed non-secret placeholder that satisfies the column check.
async fn seed_zotero(
    run: &TestRun,
    source: Uuid,
    owner: Uuid,
    document: Uuid,
    task: Uuid,
    edge: ZoteroEdge,
) {
    let admin = admin_pool(&run.harness).await;
    let connector = Uuid::now_v7();
    let reference = Uuid::now_v7();
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_connectors (id,workspace_id,owner_user_id,library_type,remote_library_id,library_url) VALUES ($1,$2,$3,'user',4242,'https://www.zotero.org/fixture/library')")
        .bind(connector).bind(source).bind(owner).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_credentials (connector_id,workspace_id,owner_user_id,sealed_key) VALUES ($1,$2,$3,'enc:v2:fixture-not-a-key')")
        .bind(connector).bind(source).bind(owner).execute(&mut *tx).await.unwrap();
    let reference_document = matches!(edge, ZoteroEdge::ReferenceOnDocument).then_some(document);
    sqlx::query("INSERT INTO fvoci.zotero_references (id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,bibliography,return_url,availability) VALUES ($1,$2,$3,$4,$5,'ABCD2345',7,'{\"title\":\"비공개 서지\"}'::jsonb,'zotero://select/library/items/ABCD2345','available')")
        .bind(reference).bind(source).bind(owner).bind(connector).bind(reference_document).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_collections (workspace_id,owner_user_id,connector_id,collection_key,remote_version,name) VALUES ($1,$2,$3,'CXLN2345',3,'비공개 컬렉션')")
        .bind(source).bind(owner).bind(connector).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_memberships (workspace_id,owner_user_id,connector_id,reference_id,collection_key) VALUES ($1,$2,$3,$4,'CXLN2345')")
        .bind(source).bind(owner).bind(connector).bind(reference).execute(&mut *tx).await.unwrap();
    let (link_document, link_task) = match edge {
        ZoteroEdge::ReferenceOnDocument => (None, None),
        ZoteroEdge::LinkOnDocument => (Some(document), None),
        ZoteroEdge::LinkOnTask => (None, Some(task)),
    };
    if link_document.is_some() || link_task.is_some() {
        sqlx::query("INSERT INTO fvoci.zotero_links (id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor) VALUES ($1,$2,$3,$4,$5,$6,$7,'p1')")
            .bind(Uuid::now_v7()).bind(source).bind(owner).bind(connector).bind(reference).bind(link_document).bind(link_task).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    admin.close().await;
}

/// MOVE refuses before any effect at preview and at commit (with a preview
/// taken before the rows existed), leaving every graph unchanged.
async fn move_refuses_with(edge: ZoteroEdge) {
    let mut run = TestRun::new(TestDb::bootstrap().await);
    let (addr, actor, source, _, _, selection) = setup_transfer(&mut run).await;
    let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
    let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
    // A preview taken before the rows exist: the commit path must
    // refuse on its own current read, not trust the preview.
    let stale = command(addr, &actor, source, &selection).await;
    seed_zotero(&run, source, actor.user_id, document, task, edge).await;
    let before = transfer_graph(&run, source).await;
    let zotero_before = zotero_graph(&run, source).await;
    let target_before = transfer_graph(&run, actor.workspace_id).await;

    let (status, error) = session_call(
        addr,
        Method::POST,
        &format!("{}/preview", path(source)),
        &actor.session_token,
        Some(selection.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{edge:?} preview: {error}");
    assert_eq!(error["code"], "personal_transfer_incomplete");
    assert_eq!(error["params"]["code"], "dependent_graph");

    let (status, error) = session_call(
        addr,
        Method::POST,
        &path(source),
        &actor.session_token,
        Some(stale),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{edge:?} commit: {error}");
    assert_eq!(error["params"]["code"], "dependent_graph");

    assert_eq!(transfer_graph(&run, source).await, before, "{edge:?}");
    assert_eq!(zotero_graph(&run, source).await, zotero_before, "{edge:?}");
    assert_eq!(
        transfer_graph(&run, actor.workspace_id).await,
        target_before,
        "{edge:?}"
    );
    assert_eq!(
        zotero_graph(&run, actor.workspace_id).await,
        zotero_graph_empty(),
        "{edge:?}"
    );
    run.finish().await.unwrap();
}

#[tokio::test]
async fn personal_transfer_move_refuses_zotero_reference_on_document() {
    run_test(
        "personal_transfer_move_refuses_zotero_reference_on_document",
        move_refuses_with(ZoteroEdge::ReferenceOnDocument),
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_move_refuses_zotero_link_on_document() {
    run_test(
        "personal_transfer_move_refuses_zotero_link_on_document",
        move_refuses_with(ZoteroEdge::LinkOnDocument),
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_move_refuses_zotero_link_on_task() {
    run_test(
        "personal_transfer_move_refuses_zotero_link_on_task",
        move_refuses_with(ZoteroEdge::LinkOnTask),
    )
    .await;
}

#[tokio::test]
async fn personal_transfer_copy_carries_no_private_zotero_rows() {
    run_test(
        "personal_transfer_copy_carries_no_private_zotero_rows",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, actor, source, _, _, mut selection) = setup_transfer(&mut run).await;
            selection["action"] = json!("copy");
            let document = Uuid::parse_str(selection["documentId"].as_str().unwrap()).unwrap();
            let task = Uuid::parse_str(selection["taskId"].as_str().unwrap()).unwrap();
            seed_zotero(&run, source, actor.user_id, document, task, ZoteroEdge::ReferenceOnDocument).await;
            seed_zotero_second_link(&run, source, actor.user_id, document, task).await;
            let zotero_before = zotero_graph(&run, source).await;
            let body = command(addr, &actor, source, &selection).await;
            let (status, copy) = session_call(
                addr,
                Method::POST,
                &path(source),
                &actor.session_token,
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{copy}");
            assert_ne!(copy["documentId"], document.to_string());
            assert_ne!(copy["taskId"], task.to_string());
            assert_eq!(zotero_graph(&run, source).await, zotero_before);
            assert_eq!(
                zotero_graph(&run, actor.workspace_id).await,
                zotero_graph_empty(),
                "COPY must not carry connectors, credentials, references, collections, memberships or links"
            );
            run.finish().await.unwrap();
        },
    )
    .await;
}

/// A second connector whose links point at both the document and the task.
async fn seed_zotero_second_link(
    run: &TestRun,
    source: Uuid,
    owner: Uuid,
    document: Uuid,
    task: Uuid,
) {
    let admin = admin_pool(&run.harness).await;
    let connector = Uuid::now_v7();
    let reference = Uuid::now_v7();
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_connectors (id,workspace_id,owner_user_id,library_type,remote_library_id,library_url) VALUES ($1,$2,$3,'group',4343,'https://www.zotero.org/groups/fixture')")
        .bind(connector).bind(source).bind(owner).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO fvoci.zotero_references (id,workspace_id,owner_user_id,connector_id,item_key,remote_version,bibliography,return_url,availability) VALUES ($1,$2,$3,$4,'WXYZ2345',2,'{}'::jsonb,'zotero://select/groups/items/WXYZ2345','available')")
        .bind(reference).bind(source).bind(owner).bind(connector).execute(&mut *tx).await.unwrap();
    for (doc, tsk) in [(Some(document), None), (None, Some(task))] {
        sqlx::query("INSERT INTO fvoci.zotero_links (id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(Uuid::now_v7()).bind(source).bind(owner).bind(connector).bind(reference).bind(doc).bind(tsk).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    admin.close().await;
}

fn zotero_graph_empty() -> Value {
    json!({
        "zotero_connectors": [],
        "zotero_credentials": [],
        "zotero_references": [],
        "zotero_collections": [],
        "zotero_memberships": [],
        "zotero_links": [],
    })
}
