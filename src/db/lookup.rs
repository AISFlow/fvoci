use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::projects::project_member_role;
use crate::db::workspace::{membership_role, workspace_is_live};
use crate::display_id::{format_display_id, parse_display_id, ParsedDisplayId};
use crate::projects::{effective_permission, ProjectPermission};
use sqlx::PgPool;

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

#[derive(Debug, Clone)]
pub struct LookupItemRow {
    pub kind: String,
    pub id: Uuid,
    pub display_id: String,
    pub title: String,
    pub project_id: Option<Uuid>,
}

pub async fn lookup_display_id(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    display_id: &str,
    project_filter: Option<Uuid>,
) -> Result<Result<Vec<LookupItemRow>, LookupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(LookupDbError::NotFound));
    };
    let parsed = parse_display_id(display_id);
    if parsed.is_none() {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }
    let ParsedDisplayId { prefix, number } = parsed.unwrap();
    if prefix == "WIKI" {
        if project_filter.is_some() {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        }
        let doc: Option<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT id, title
            FROM fvoci.documents
            WHERE workspace_id = $1 AND project_id IS NULL AND number = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id, title)) = doc else {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        };
        let permission = crate::db::documents::document_permission(
            &mut tx,
            workspace_id,
            actor_user_id,
            id,
            true,
        )
        .await?;
        if !permission.at_least(ProjectPermission::View) {
            tx.rollback().await?;
            return Ok(Ok(Vec::new()));
        }
        tx.commit().await?;
        return Ok(Ok(vec![LookupItemRow {
            kind: "document".to_string(),
            id,
            display_id: format_display_id("WIKI", number),
            title,
            project_id: None,
        }]));
    }

    let project: Option<(Uuid, String, String)> = sqlx::query_as(
        r#"
        SELECT id, key, visibility
        FROM fvoci.projects
        WHERE workspace_id = $1 AND key = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(&prefix)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, project_key, visibility)) = project else {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    };
    // An invisible project and a filter mismatch both answer an empty list
    // (not 404), so the response does not reveal whether the key exists.
    if project_filter.is_some_and(|filter| filter != project_id) {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }
    let member_role = project_member_role(&mut tx, workspace_id, project_id, actor_user_id).await?;
    let permission = effective_permission(role, &visibility, member_role);
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Ok(Vec::new()));
    }

    let label = format_display_id(&project_key, number);
    let doc: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, title
        FROM fvoci.documents
        WHERE workspace_id = $1 AND project_id = $2 AND number = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(number)
    .fetch_optional(&mut *tx)
    .await?;
    let task: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, title
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND project_id = $2 AND number = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(number)
    .fetch_optional(&mut *tx)
    .await?;

    let mut items = Vec::new();
    if let Some((id, title)) = doc {
        items.push(LookupItemRow {
            kind: "document".to_string(),
            id,
            display_id: label.clone(),
            title,
            project_id: Some(project_id),
        });
    }
    if let Some((id, title)) = task {
        items.push(LookupItemRow {
            kind: "task".to_string(),
            id,
            display_id: label,
            title,
            project_id: Some(project_id),
        });
    }
    tx.commit().await?;
    Ok(Ok(items))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupDbError {
    NotFound,
    Forbidden,
}

/// PostgreSQL keeps its original READ COMMITTED consumer. Family lookup reads
/// current credentials, grants and returned rows in one read snapshot.
pub async fn lookup_display_id_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    display_id: &str,
    project_filter: Option<Uuid>,
) -> Result<Result<Vec<LookupItemRow>, LookupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return lookup_display_id(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            display_id,
            project_filter,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = match &mut tx {
        DbTx::SqliteFamily(family) => {
            lookup_display_id_family(
                family,
                workspace_id,
                actor_user_id,
                session_id,
                display_id,
                project_filter,
            )
            .await
        }
        DbTx::Postgres(_) => Err(sqlx::Error::Protocol(
            "family lookup requires selected family transaction".into(),
        )),
    };
    match result {
        Ok(Ok(items)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(items))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(LookupReadRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("lookup read refused: {0:?}")]
struct LookupReadRefusal(LookupDbError);

fn lookup_identity(row: &FamilyRow) -> Result<(Uuid, String), sqlx::Error> {
    Ok((row.cell(0)?.id()?, row.cell(1)?.string()?))
}

async fn lookup_display_id_family(
    tx: &mut FamilyTx,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    display_id: &str,
    project_filter: Option<Uuid>,
) -> Result<Result<Vec<LookupItemRow>, LookupDbError>, sqlx::Error> {
    let mut op = OperationTx::SqliteFamily(&mut *tx);
    op.set_tenant(workspace).await?;
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(LookupDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await?
        || op.membership_role(workspace, actor, false).await?.is_none()
    {
        return Ok(Err(LookupDbError::NotFound));
    }
    let Some(ParsedDisplayId { prefix, number }) = parse_display_id(display_id) else {
        return Ok(Ok(Vec::new()));
    };
    if prefix == "WIKI" {
        if project_filter.is_some() {
            return Ok(Ok(Vec::new()));
        }
        let rows = tx
            .query(
                "SELECT id,title FROM documents WHERE workspace_id=?1 AND project_id IS NULL AND number=?2 AND deleted_at IS NULL",
                &[Cell::uuid(workspace), Cell::Integer(i64::from(number))],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Ok(Vec::new()));
        };
        let (id, title) = lookup_identity(row)?;
        if !OperationTx::SqliteFamily(tx)
            .document_permission(workspace, actor, id, true)
            .await?
            .at_least(ProjectPermission::View)
        {
            return Ok(Ok(Vec::new()));
        }
        return Ok(Ok(vec![LookupItemRow {
            kind: "document".into(),
            id,
            display_id: format_display_id("WIKI", number),
            title,
            project_id: None,
        }]));
    }
    let rows = tx
        .query(
            "SELECT id,key FROM projects WHERE workspace_id=?1 AND key=?2 AND deleted_at IS NULL",
            &[Cell::uuid(workspace), Cell::Text(prefix)],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(Ok(Vec::new()));
    };
    let (project, key) = lookup_identity(row)?;
    if project_filter.is_some_and(|filter| filter != project)
        || !OperationTx::SqliteFamily(tx)
            .project_permission_by_id(workspace, actor, project)
            .await?
            .is_some_and(|permission| permission.at_least(ProjectPermission::View))
    {
        return Ok(Ok(Vec::new()));
    }
    let args = [
        Cell::uuid(workspace),
        Cell::uuid(project),
        Cell::Integer(i64::from(number)),
    ];
    let documents = tx
        .query(
            "SELECT id,title FROM documents WHERE workspace_id=?1 AND project_id=?2 AND number=?3 AND deleted_at IS NULL",
            &args,
        )
        .await?;
    let tasks = tx
        .query(
            "SELECT id,title FROM tasks WHERE workspace_id=?1 AND project_id=?2 AND number=?3 AND deleted_at IS NULL",
            &args,
        )
        .await?;
    let label = format_display_id(&key, number);
    let mut items = Vec::new();
    // Preserve both possible variants and their original document/task order.
    for (kind, rows) in [("document", documents), ("task", tasks)] {
        if let Some(row) = rows.first() {
            let (id, title) = lookup_identity(row)?;
            items.push(LookupItemRow {
                kind: kind.into(),
                id,
                display_id: label.clone(),
                title,
                project_id: Some(project),
            });
        }
    }
    Ok(Ok(items))
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod selected_lookup_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::task_origins::{
        create_document_task_backend, origin_request_hash, DocumentTaskOutcome, DocumentTaskRequest,
    };
    use crate::db::tasks::CreateTaskInput;
    use serde_json::json;

    pub(crate) async fn session(f: &Fixture, user: Uuid) -> (Uuid, String) {
        let token = crate::auth::token::new_token();
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (id, token.token)
    }

    pub(crate) async fn project(f: &Fixture, credential: Uuid) -> Uuid {
        crate::db::projects::create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            crate::db::projects::CreateProjectInput {
                key: "ORIGIN",
                name: "원본 프로젝트 中 😀",
                visibility: "private",
                description: None,
                icon: None,
                lead_user_id: None,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id
    }

    pub(crate) async fn origin_task(
        f: &Fixture,
        credential: Uuid,
        project: Uuid,
        command: Uuid,
    ) -> DocumentTaskOutcome {
        let dto: crate::api::dto::CreateTaskBody =
            serde_json::from_value(json!({"title":"실제 원본 작업 中 😀"})).unwrap();
        let hash = origin_request_hash(
            f.user,
            project,
            Some("literal-source-block"),
            &crate::http::routes::task_body::normalized_task_input(&dto),
        );
        create_document_task_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            DocumentTaskRequest {
                document_id: f.document,
                project_id: project,
                request_id: command,
                anchor: Some("literal-source-block"),
                request_hash: &hash,
                self_assign: false,
                task: CreateTaskInput {
                    title: &dto.title,
                    task_type: &dto.task_type,
                    priority: &dto.priority,
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: None,
                    milestone_id: None,
                    recurrence: None,
                },
            },
            Some("127.0.0.1"),
            "api",
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn lookup(
        f: &Fixture,
        actor: Uuid,
        credential: Uuid,
        display: &str,
    ) -> Vec<LookupItemRow> {
        lookup_display_id_backend(&f.backend, f.workspace, actor, credential, display, None)
            .await
            .unwrap()
            .unwrap()
    }

    async fn user(f: &Fixture, role: &str) -> (Uuid, Uuid) {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Lookup')")
            .bind(id.as_bytes().as_slice())
            .bind(format!("{id}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(id.as_bytes().as_slice())
            .bind(role)
            .execute(&f.pool)
            .await
            .unwrap();
        (id, session(f, id).await.0)
    }

    #[tokio::test]
    async fn wiki_aux_lookup_origin_task_wiki_project_document_literal_new_client() {
        let f = Fixture::new().await;
        let credential = session(&f, f.user).await.0;
        let project = project(&f, credential).await;
        let command = Uuid::now_v7();
        let first = origin_task(&f, credential, project, command).await;
        let DocumentTaskOutcome::Created(task) = first else {
            panic!("fresh logical command must create")
        };
        assert_eq!(
            origin_task(&f, credential, project, command).await,
            DocumentTaskOutcome::Replayed(task)
        );
        let client = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let items = lookup_display_id_backend(
            &Backend::Sqlite(client.clone()),
            f.workspace,
            f.user,
            credential,
            "origin-002",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "task");
        assert_eq!(items[0].id, task);
        assert_eq!(items[0].title, "실제 원본 작업 中 😀");
        assert_eq!(items[0].display_id, "ORIGIN-2");
        assert_eq!(items[0].project_id, Some(project));
        let root: Vec<u8> = sqlx::query_scalar("SELECT root_document_id FROM projects WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let docs = lookup(&f, f.user, credential, "ORIGIN-1").await;
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].kind, "document");
        assert_eq!(docs[0].id, Uuid::from_slice(&root).unwrap());
        assert_eq!(docs[0].project_id, Some(project));
        assert_eq!(docs[0].display_id, "ORIGIN-1");
        let wiki = lookup(&f, f.user, credential, " WIKI-1 ").await;
        assert_eq!(wiki.len(), 1);
        assert_eq!(wiki[0].kind, "document");
        assert_eq!(wiki[0].id, f.document);
        assert_eq!(wiki[0].title, "S31");
        assert_eq!(wiki[0].project_id, None);
        assert_eq!(wiki[0].display_id, "WIKI-1");
        let publication: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM tasks),(SELECT count(*) FROM task_origins),(SELECT next_number FROM projects WHERE id=?1)",
        )
        .bind(project.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(publication, (1, 1, 3));
        client.close().await;
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_lookup_current_private_direct_group_wiki_grants_and_revoke() {
        let f = Fixture::new().await;
        let credential = session(&f, f.user).await.0;
        let project = project(&f, credential).await;
        let task = origin_task(&f, credential, project, Uuid::now_v7())
            .await
            .task_id();
        let (actor, current) = user(&f, "owner").await;
        assert!(lookup(&f, actor, current, "ORIGIN-2").await.is_empty());
        assert!(lookup(&f, actor, current, "MISSING-2").await.is_empty());
        let direct = Uuid::now_v7();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(direct.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(lookup(&f, actor, current, "ORIGIN-2").await[0].id, task);
        sqlx::query("DELETE FROM project_members WHERE id=?1")
            .bind(direct.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(lookup(&f, actor, current, "ORIGIN-2").await.is_empty());
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Lookup grant')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(lookup(&f, actor, current, "ORIGIN-2").await[0].id, task);
        // Wiki guest visibility uses the canonical group grant, not a fabricated
        // direct-user document grant that the maintained resolver ignores.
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(lookup(&f, actor, current, "WIKI-1").await.is_empty());
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(lookup(&f, actor, current, "WIKI-1").await[0].id, f.document);
        let mut revoker = f.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query(
            "DELETE FROM group_members WHERE workspace_id=?1 AND group_id=?2 AND user_id=?3",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(group.as_bytes().as_slice())
        .bind(actor.as_bytes().as_slice())
        .execute(&mut *revoker)
        .await
        .unwrap();
        revoker.commit().await.unwrap();
        assert!(lookup(&f, actor, current, "ORIGIN-2").await.is_empty());
        assert!(lookup(&f, actor, current, "WIKI-1").await.is_empty());
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(lookup(&f, actor, current, "ORIGIN-2").await[0].id, task);
        assert_eq!(lookup(&f, actor, current, "WIKI-1").await[0].id, f.document);
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_lookup_current_credentials_membership_workspace_tenant_denial() {
        let f = Fixture::new().await;
        let credential = session(&f, f.user).await.0;
        let (other, other_credential) = user(&f, "member").await;
        let read = |workspace, actor, credential| {
            let backend = &f.backend;
            async move {
                lookup_display_id_backend(backend, workspace, actor, credential, "WIKI-1", None)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            read(f.workspace, other, credential).await.unwrap_err(),
            LookupDbError::Forbidden
        );
        assert_eq!(
            read(Uuid::now_v7(), f.user, credential).await.unwrap_err(),
            LookupDbError::NotFound
        );
        let foreign = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,name,slug) VALUES(?1,'Foreign',?2)")
            .bind(foreign.as_bytes().as_slice())
            .bind(foreign.simple().to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            read(foreign, f.user, credential).await.unwrap_err(),
            LookupDbError::NotFound
        );
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(foreign.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(read(foreign, f.user, credential).await.unwrap().is_empty());
        let token = Uuid::now_v7();
        let material = crate::auth::token::new_token();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Lookup tenant','[\"documents.read\"]')")
            .bind(token.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&material.hash).execute(&f.pool).await.unwrap();
        assert_eq!(
            read(f.workspace, f.user, token).await.unwrap()[0].id,
            f.document
        );
        assert_eq!(
            read(foreign, f.user, token).await.unwrap_err(),
            LookupDbError::Forbidden
        );
        let live_expiry = (chrono::Utc::now().timestamp_micros() + 86_400_000_000).to_string();
        for (column, dead, restored) in [
            ("revoked_at", "1", "NULL"),
            ("expires_at", "1", live_expiry.as_str()),
        ] {
            // Static test-only alternatives; no product dynamic SQL path.
            let sql = format!("UPDATE sessions SET {column}={dead} WHERE id=?1");
            sqlx::query(&sql)
                .bind(credential.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                read(f.workspace, f.user, credential).await.unwrap_err(),
                LookupDbError::Forbidden
            );
            let sql = format!("UPDATE sessions SET {column}={restored} WHERE id=?1");
            sqlx::query(&sql)
                .bind(credential.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        for column in ["suspended_at", "deleted_at"] {
            let sql = format!("UPDATE users SET {column}=1 WHERE id=?1");
            sqlx::query(&sql)
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                read(f.workspace, f.user, credential).await.unwrap_err(),
                LookupDbError::Forbidden
            );
            let sql = format!("UPDATE users SET {column}=NULL WHERE id=?1");
            sqlx::query(&sql)
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            read(f.workspace, other, other_credential)
                .await
                .unwrap_err(),
            LookupDbError::NotFound
        );
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            read(f.workspace, f.user, credential).await.unwrap_err(),
            LookupDbError::NotFound
        );
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            read(f.workspace, f.user, credential).await.unwrap()[0].id,
            f.document
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_lookup_parser_filter_collision_order_deleted_archived_contract() {
        let f = Fixture::new().await;
        let credential = session(&f, f.user).await.0;
        let project = project(&f, credential).await;
        let task = origin_task(&f, credential, project, Uuid::now_v7())
            .await
            .task_id();
        for invalid in [
            "bad",
            "X-1",
            "ORIGIN-1234567890",
            "ORIGIN--1",
            "MISSING-2",
            "ORIGIN-0",
        ] {
            assert!(
                lookup(&f, f.user, credential, invalid).await.is_empty(),
                "{invalid}"
            );
        }
        for (display, filter, expected) in [
            ("ORIGIN-2", project, 1),
            ("ORIGIN-2", Uuid::now_v7(), 0),
            ("WIKI-1", project, 0),
        ] {
            assert_eq!(
                lookup_display_id_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    display,
                    Some(filter)
                )
                .await
                .unwrap()
                .unwrap()
                .len(),
                expected
            );
        }
        let document = Uuid::now_v7();
        let root: Vec<u8> = sqlx::query_scalar("SELECT root_document_id FROM projects WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let root = Uuid::from_slice(&root).unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,project_id,parent_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,?3,?4,'Collision document',?5,'V',2,'published',2,?6,'{\"type\":\"doc\",\"content\":[]}')")
            .bind(document.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(root.as_bytes().as_slice()).bind(format!("{}.{}",root.simple(),document.simple())).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let items = lookup(&f, f.user, credential, " origin-002 ").await;
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].kind.as_str(), items[0].id, items[0].title.as_str()),
            ("document", document, "Collision document")
        );
        assert_eq!((items[1].kind.as_str(), items[1].id), ("task", task));
        assert!(items
            .iter()
            .all(|row| row.display_id == "ORIGIN-2" && row.project_id == Some(project)));
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(lookup(&f, f.user, credential, "ORIGIN-2").await.len(), 2);
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(lookup(&f, f.user, credential, "ORIGIN-2").await[0].id, task);
        sqlx::query("UPDATE tasks SET deleted_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(lookup(&f, f.user, credential, "ORIGIN-2").await.is_empty());
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(lookup(&f, f.user, credential, "ORIGIN-1").await.is_empty());
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(lookup(&f, f.user, credential, "WIKI-1").await.is_empty());
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}
