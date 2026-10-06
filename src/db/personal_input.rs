//! Atomic personal input into ordinary entities, with an append-only retry receipt.
use crate::api::personal_input_dto::{PersonalInputBody, PersonalInputIntent, PersonalInputOutput};
use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::Cell;
use crate::db::context::{lock_membership_users, lock_tree, recheck_session, set_tenant};
use crate::db::documents::{create_wiki_document_tx, CreateDocumentInput};
use crate::db::projects::{
    create_project_tx, project_permission_by_id, CreateProjectInput, ProjectDbError,
};
use crate::db::task_origins::{
    create_document_task_tx, document_view_permission, origin_request_hash, task_view_permission,
    DocumentTaskRequest, TaskOriginDbError,
};
use crate::db::tasks::CreateTaskInput;
use crate::projects::ProjectPermission;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// Select the real backend without changing the legacy PostgreSQL entry point.
/// One writer owns the ordinary entities, assignment, origin and retry receipt.
pub async fn create_personal_input_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
    input: &PersonalInputBody,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_personal_input(
            pool,
            workspace_id,
            actor,
            session_id,
            input,
            client_ip,
            channel,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = create_family_input(
        &mut tx.operation(),
        workspace_id,
        actor,
        session_id,
        input,
        client_ip,
        channel,
    )
    .await;
    finish_personal_input(tx, result).await
}

#[derive(Debug, thiserror::Error)]
#[error("personal input refused: {0:?}")]
struct PersonalInputRefusal(PersonalInputDbError);

async fn finish_personal_input(
    tx: DbTx,
    result: Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error>,
) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
    match result {
        Ok(Ok(output)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(output))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(PersonalInputRefusal(refusal))),
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

async fn create_family_input(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    input: &PersonalInputBody,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
    op.set_tenant(workspace).await?;
    op.lock_membership_users(&[actor]).await?;
    if !op.recheck_session(actor, credential).await? {
        return Ok(Err(PersonalInputDbError::Forbidden));
    }
    if !op.origin_personal_workspace_owner(workspace, actor).await? {
        return Ok(Err(PersonalInputDbError::NotFound));
    }
    op.lock_tree(workspace).await?;
    let hash = request_hash(workspace, actor, input);
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    family.require_writer()?;
    family.require_tenant(workspace)?;
    let receipts = family.query(
        "SELECT request_hash,document_id,task_id,project_id FROM personal_input_commands WHERE workspace_id=?1 AND actor_user_id=?2 AND request_id=?3",
        &[Cell::uuid(workspace),Cell::uuid(actor),Cell::uuid(input.request_id)],
    ).await?;
    if let Some(receipt) = receipts.first() {
        if receipt.cell(0)?.string()? != hash {
            return Ok(Err(PersonalInputDbError::RequestMismatch));
        }
        let Some(document) = receipt.cell(1)?.optional(Cell::id)? else {
            return Ok(Err(PersonalInputDbError::NotFound));
        };
        let task = receipt.cell(2)?.optional(Cell::id)?;
        let project = receipt.cell(3)?.optional(Cell::id)?;
        if crate::db::task_origins::origin_write_source(op, workspace, actor, document)
            .await?
            .is_none()
        {
            return Ok(Err(PersonalInputDbError::NotFound));
        }
        if input.intent == PersonalInputIntent::Task {
            let Some(task) = task else {
                return Ok(Err(PersonalInputDbError::NotFound));
            };
            if !op
                .origin_task_view_permission(workspace, actor, task)
                .await?
                .at_least(ProjectPermission::View)
            {
                return Ok(Err(PersonalInputDbError::NotFound));
            }
        }
        return Ok(Ok(family_output(
            op, workspace, document, task, project, true,
        )
        .await?));
    }
    let project = if input.intent == PersonalInputIntent::Task {
        Some(match input.project_id {
            Some(id) => id,
            None => match family_default_project(
                op,
                workspace,
                actor,
                credential,
                input.request_id,
                client_ip,
            )
            .await?
            {
                Ok(id) => id,
                Err(error) => return Ok(Err(PersonalInputDbError::Project(error))),
            },
        })
    } else {
        None
    };
    let document = match &input.source {
        Some(source) => source.document_id,
        None => match crate::db::documents::create_wiki_document_operation(
            op,
            workspace,
            actor,
            credential,
            CreateDocumentInput {
                parent_id: None,
                title: input.title.trim(),
                icon: None,
            },
            client_ip,
            None,
        )
        .await?
        {
            Ok(Some(document)) => document.id,
            _ => return Ok(Err(PersonalInputDbError::NotFound)),
        },
    };
    let task = if let Some(project) = project {
        let anchor = input
            .source
            .as_ref()
            .and_then(|source| source.anchor.as_deref());
        // Preserve the original typed origin hash, command UUID and self-assignment.
        let normalized = serde_json::json!({"title":input.title,"type":"task","priority":"medium","statusId":null,"startDate":null,"dueDate":null,"parentId":null,"milestoneId":null,"recurrence":null,"selfAssign":true});
        let origin_hash = origin_request_hash(actor, project, anchor, &normalized);
        match crate::db::task_origins::create_document_task_operation(
            op,
            workspace,
            actor,
            credential,
            DocumentTaskRequest {
                document_id: document,
                project_id: project,
                request_id: input.request_id,
                self_assign: true,
                anchor,
                request_hash: &origin_hash,
                task: CreateTaskInput {
                    title: input.title.trim(),
                    task_type: "task",
                    priority: "medium",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: None,
                    milestone_id: None,
                    recurrence: None,
                },
            },
            client_ip,
            channel,
        )
        .await?
        {
            Ok(outcome) => Some(outcome.task_id()),
            Err(error) => return Ok(Err(PersonalInputDbError::Origin(error))),
        }
    } else {
        None
    };
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    family.execute(
        "INSERT INTO personal_input_commands(workspace_id,actor_user_id,request_id,request_hash,intent,document_id,task_id,project_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        &[Cell::uuid(workspace),Cell::uuid(actor),Cell::uuid(input.request_id),Cell::text(hash),Cell::text(input.intent.as_str()),Cell::uuid(document),Cell::optional_uuid(task),Cell::optional_uuid(project)],
    ).await?;
    Ok(Ok(family_output(
        op, workspace, document, task, project, false,
    )
    .await?))
}

async fn family_default_project(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    request: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    let candidates = family.query(
        "SELECT id FROM projects WHERE workspace_id=?1 AND (key='INBOX' OR key LIKE 'INBOX-%') AND visibility='private' AND deleted_at IS NULL AND status='active' ORDER BY (key='INBOX') DESC,key COLLATE BINARY,id",
        &[Cell::uuid(workspace)],
    ).await?;
    for candidate in candidates {
        let id = candidate.cell(0)?.id()?;
        if op
            .project_permission_by_id(workspace, actor, id)
            .await?
            .is_some_and(|permission| permission.at_least(ProjectPermission::Edit))
        {
            return Ok(Ok(id));
        }
    }
    // Include archived and trashed keys, just as the original bounded allocator.
    let keys = default_project_keys(request);
    let mut args = vec![Cell::uuid(workspace)];
    args.extend(keys.iter().map(|key| Cell::text(key.as_str())));
    let OperationTx::SqliteFamily(family) = &mut *op else {
        unreachable!()
    };
    let occupied = family
        .query(
            "SELECT key FROM projects WHERE workspace_id=?1 AND key IN (?2,?3,?4,?5,?6,?7,?8,?9)",
            &args,
        )
        .await?
        .iter()
        .map(|row| row.cell(0)?.string())
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let Some(key) = keys.iter().find(|key| !occupied.contains(key)) else {
        return Ok(Err(ProjectDbError::Conflict));
    };
    Ok(crate::db::projects::create_project_operation(
        op,
        workspace,
        actor,
        credential,
        CreateProjectInput {
            key,
            name: "Personal",
            visibility: "private",
            description: None,
            icon: None,
            lead_user_id: None,
        },
        client_ip,
    )
    .await?
    .map(|project| project.id))
}

async fn family_output(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    document: Uuid,
    task: Option<Uuid>,
    project: Option<Uuid>,
    replayed: bool,
) -> Result<PersonalInputOutput, sqlx::Error> {
    let OperationTx::SqliteFamily(family) = op else {
        unreachable!()
    };
    let rows = family.query(
        "SELECT COALESCE(p.key,'WIKI') || '-' || CAST(d.number AS TEXT) FROM documents d LEFT JOIN projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id WHERE d.workspace_id=?1 AND d.id=?2",
        &[Cell::uuid(workspace),Cell::uuid(document)],
    ).await?;
    let document_display_id = rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .string()?;
    let task_display_id = if let Some(task) = task {
        let rows = family.query(
            "SELECT p.key || '-' || CAST(t.number AS TEXT) FROM tasks t JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id WHERE t.workspace_id=?1 AND t.id=?2",
            &[Cell::uuid(workspace),Cell::uuid(task)],
        ).await?;
        Some(
            rows.first()
                .ok_or(sqlx::Error::RowNotFound)?
                .cell(0)?
                .string()?,
        )
    } else {
        None
    };
    Ok(PersonalInputOutput {
        document_id: document.to_string(),
        document_display_id,
        task_id: task.map(|id| id.to_string()),
        task_display_id,
        project_id: project.map(|id| id.to_string()),
        replayed,
    })
}
#[derive(Debug)]
pub enum PersonalInputDbError {
    NotFound,
    Forbidden,
    RequestMismatch,
    Project(ProjectDbError),
    Origin(TaskOriginDbError),
}

struct ReceiptRow {
    request_hash: String,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for ReceiptRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            request_hash: row.try_get("request_hash")?,
            document_id: row.try_get("document_id")?,
            task_id: row.try_get("task_id")?,
            project_id: row.try_get("project_id")?,
        })
    }
}

/// Caller owns the actor membership/session lock. Personal kind alone is not
/// authority: bind the user's linked workspace AND current owner membership.
pub(crate) async fn owns_personal_workspace(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"SELECT EXISTS (
        SELECT 1 FROM fvoci.workspaces w JOIN fvoci.users u ON u.personal_workspace_id = w.id
        JOIN fvoci.memberships m ON m.workspace_id = w.id AND m.user_id = u.id
        WHERE w.id = $1 AND w.kind = 'personal' AND w.deleted_at IS NULL
        AND u.id = $2 AND u.deleted_at IS NULL AND m.role = 'owner'
    )"#,
    )
    .bind(workspace_id)
    .bind(actor)
    .fetch_one(&mut **tx)
    .await
}

pub fn request_hash(workspace_id: Uuid, actor: Uuid, input: &PersonalInputBody) -> String {
    let canonical = serde_json::json!({"workspaceId":workspace_id,"actor":actor,"intent":input.intent,"title":input.title,"projectId":input.project_id,"source":input.source});
    Sha256::digest(canonical.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub async fn create_personal_input(
    pool: &PgPool,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
    input: &PersonalInputBody,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = create_in_tx(
        &mut tx,
        workspace_id,
        actor,
        session_id,
        input,
        client_ip,
        channel,
    )
    .await?;
    if result.is_ok() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

async fn create_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
    input: &PersonalInputBody,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor]).await?;
    if !recheck_session(tx, actor, session_id).await? {
        return Ok(Err(PersonalInputDbError::Forbidden));
    }
    if !owns_personal_workspace(tx, workspace_id, actor).await? {
        return Ok(Err(PersonalInputDbError::NotFound));
    }
    // Per-user membership serialization precedes this key; no parallel duplicate
    // can pass an absent receipt. The tenant tree lock also orders default project
    // and note creation with existing project/document writers.
    lock_tree(tx, workspace_id).await?;
    let hash = request_hash(workspace_id, actor, input);
    let receipt: Option<ReceiptRow> = sqlx::query_as(
        "SELECT request_hash, document_id, task_id, project_id FROM fvoci.personal_input_commands WHERE workspace_id = $1 AND actor_user_id = $2 AND request_id = $3"
    ).bind(workspace_id).bind(actor).bind(input.request_id).fetch_optional(&mut **tx).await?;
    if let Some(ReceiptRow {
        request_hash: original_hash,
        document_id,
        task_id,
        project_id,
    }) = receipt
    {
        if hash != original_hash {
            return Ok(Err(PersonalInputDbError::RequestMismatch));
        }
        let Some(document_id) = document_id else {
            return Ok(Err(PersonalInputDbError::NotFound));
        };
        if !document_view_permission(tx, workspace_id, actor, document_id)
            .await?
            .at_least(ProjectPermission::View)
        {
            return Ok(Err(PersonalInputDbError::NotFound));
        }
        if input.intent == PersonalInputIntent::Task {
            let Some(task_id) = task_id else {
                return Ok(Err(PersonalInputDbError::NotFound));
            };
            if !task_view_permission(tx, workspace_id, actor, task_id)
                .await?
                .at_least(ProjectPermission::View)
            {
                return Ok(Err(PersonalInputDbError::NotFound));
            }
        }
        return Ok(Ok(output(
            tx,
            workspace_id,
            document_id,
            task_id,
            project_id,
            true,
        )
        .await?));
    }
    let project_id = if input.intent == PersonalInputIntent::Task {
        match input.project_id {
            Some(id) => Some(id),
            None => {
                let candidates: Vec<Uuid> = sqlx::query_scalar(
                    r#"SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND (key='INBOX' OR key LIKE 'INBOX-%') AND visibility='private' AND deleted_at IS NULL AND status='active' ORDER BY (key='INBOX') DESC, key COLLATE "C", id"#
                ).bind(workspace_id).fetch_all(&mut **tx).await?;
                let mut existing = None;
                for id in candidates {
                    if project_permission_by_id(tx, workspace_id, actor, id)
                        .await?
                        .is_some_and(|permission| permission.at_least(ProjectPermission::Edit))
                    {
                        existing = Some(id);
                        break;
                    }
                }
                Some(match existing {
                    Some(id) => id,
                    None => {
                        // Archive/trash keep their unique keys. Inspect all occupied
                        // candidate keys under the existing tenant tree lock; never
                        // restore/purge a row or retry a failed SQL transaction.
                        let keys = default_project_keys(input.request_id);
                        let occupied: Vec<String> = sqlx::query_scalar(
                            "SELECT key FROM fvoci.projects WHERE workspace_id=$1 AND key=ANY($2)",
                        )
                        .bind(workspace_id)
                        .bind(&keys[..])
                        .fetch_all(&mut **tx)
                        .await?;
                        let Some(key) = keys.iter().find(|key| !occupied.contains(key)) else {
                            return Ok(Err(PersonalInputDbError::Project(
                                ProjectDbError::Conflict,
                            )));
                        };
                        match create_project_tx(
                            tx,
                            workspace_id,
                            actor,
                            session_id,
                            CreateProjectInput {
                                key,
                                name: "Personal",
                                visibility: "private",
                                description: None,
                                icon: None,
                                lead_user_id: None,
                            },
                            client_ip,
                        )
                        .await?
                        {
                            Ok(project) => project.id,
                            Err(err) => return Ok(Err(PersonalInputDbError::Project(err))),
                        }
                    }
                })
            }
        }
    } else {
        None
    };
    let document_id = match &input.source {
        Some(source) => source.document_id,
        None => match create_wiki_document_tx(
            tx,
            workspace_id,
            actor,
            session_id,
            CreateDocumentInput {
                parent_id: None,
                title: input.title.trim(),
                icon: None,
            },
            client_ip,
            None,
        )
        .await?
        {
            Ok(Some(document)) => document.id,
            _ => return Ok(Err(PersonalInputDbError::NotFound)),
        },
    };
    let task_id = if let Some(project_id) = project_id {
        let anchor = input
            .source
            .as_ref()
            .and_then(|source| source.anchor.as_deref());
        // The normal origin command's UUID and transaction are reused; assignment
        // is part of that transaction and is never a later optimistic PATCH.
        let normalized = serde_json::json!({"title":input.title,"type":"task","priority":"medium","statusId":null,"startDate":null,"dueDate":null,"parentId":null,"milestoneId":null,"recurrence":null,"selfAssign":true});
        let origin_hash = origin_request_hash(actor, project_id, anchor, &normalized);
        match create_document_task_tx(
            tx,
            workspace_id,
            actor,
            session_id,
            DocumentTaskRequest {
                document_id,
                project_id,
                request_id: input.request_id,
                self_assign: true,
                anchor,
                request_hash: &origin_hash,
                task: CreateTaskInput {
                    title: input.title.trim(),
                    task_type: "task",
                    priority: "medium",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: None,
                    milestone_id: None,
                    recurrence: None,
                },
            },
            client_ip,
            channel,
        )
        .await?
        {
            Ok(outcome) => Some(outcome.task_id()),
            Err(err) => return Ok(Err(PersonalInputDbError::Origin(err))),
        }
    } else {
        None
    };
    sqlx::query("INSERT INTO fvoci.personal_input_commands (workspace_id, actor_user_id, request_id, request_hash, intent, document_id, task_id, project_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(workspace_id).bind(actor).bind(input.request_id).bind(hash).bind(input.intent.as_str()).bind(document_id).bind(task_id).bind(project_id).execute(&mut **tx).await?;
    Ok(Ok(output(
        tx,
        workspace_id,
        document_id,
        task_id,
        project_id,
        false,
    )
    .await?))
}
async fn output(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    task_id: Option<Uuid>,
    project_id: Option<Uuid>,
    replayed: bool,
) -> Result<PersonalInputOutput, sqlx::Error> {
    let document_display_id: String = sqlx::query_scalar("SELECT COALESCE(p.key, 'WIKI') || '-' || d.number::text FROM fvoci.documents d LEFT JOIN fvoci.projects p ON p.workspace_id = d.workspace_id AND p.id = d.project_id WHERE d.workspace_id = $1 AND d.id = $2")
        .bind(workspace_id).bind(document_id).fetch_one(&mut **tx).await?;
    let task_display_id = match task_id {
        Some(task) => Some(sqlx::query_scalar::<_, String>("SELECT p.key || '-' || t.number::text FROM fvoci.tasks t JOIN fvoci.projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id WHERE t.workspace_id = $1 AND t.id = $2").bind(workspace_id).bind(task).fetch_one(&mut **tx).await?),
        None => None,
    };
    Ok(PersonalInputOutput {
        document_id: document_id.to_string(),
        document_display_id,
        task_id: task_id.map(|id| id.to_string()),
        task_display_id,
        project_id: project_id.map(|id| id.to_string()),
        replayed,
    })
}

fn default_project_keys(request_id: Uuid) -> [String; 8] {
    // Letter before the 24-byte UUID suffix keeps every key within the existing
    // <=32-character project grammar, including the forbidden -digits suffix.
    let suffix = request_id.simple().to_string()[8..].to_uppercase();
    [
        "INBOX".into(),
        "INBOX-A".into(),
        "INBOX-B".into(),
        "INBOX-C".into(),
        format!("INBOX-D{suffix}"),
        format!("INBOX-E{suffix}"),
        format!("INBOX-F{suffix}"),
        format!("INBOX-G{suffix}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_default_keys_obey_existing_project_grammar() {
        for request in [Uuid::nil(), Uuid::now_v7(), Uuid::from_bytes([255; 16])] {
            let keys = default_project_keys(request);
            let mut unique = std::collections::HashSet::new();
            for key in keys {
                assert_eq!(
                    crate::projects::normalize_project_key(&key),
                    Ok(key.clone())
                );
                assert!(unique.insert(key));
            }
            assert_eq!(unique.len(), 8);
        }
    }
    #[test]
    fn hash_binds_actor_intent_source_and_original_typed_title() {
        let ws = Uuid::nil();
        let actor = Uuid::now_v7();
        let mut input = PersonalInputBody {
            request_id: Uuid::now_v7(),
            intent: PersonalInputIntent::Note,
            title: " 한글 🙂 ".into(),
            project_id: None,
            source: None,
        };
        let hash = request_hash(ws, actor, &input);
        input.title = "한글 🙂".into();
        assert_ne!(hash, request_hash(ws, actor, &input));
        input.title = " 한글 🙂 ".into();
        assert_ne!(hash, request_hash(ws, Uuid::now_v7(), &input));
        input.intent = PersonalInputIntent::Task;
        assert_ne!(hash, request_hash(ws, actor, &input));
        input.intent = PersonalInputIntent::Note;
        input.title = "changed".into();
        assert_ne!(hash, request_hash(ws, actor, &input));
    }
}

#[cfg(test)]
pub(crate) mod selected_personal_input_tests {
    use super::*;
    use crate::api::personal_input_dto::PersonalInputSource;
    use crate::db::attachment_preview::tests::Fixture;
    use serde_json::{json, Value};

    pub(crate) async fn setup() -> (Fixture, Uuid) {
        let f = Fixture::new().await;
        sqlx::query("UPDATE workspaces SET kind='personal',next_wiki_number=2 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET personal_workspace_id=?1 WHERE id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let credential = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                "personal-input-test",
                crate::db::identity::stored_now()
                    + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (f, credential)
    }
    pub(crate) fn input(intent: PersonalInputIntent) -> PersonalInputBody {
        PersonalInputBody {
            request_id: Uuid::now_v7(),
            intent,
            title: "  개인 입력 中 😀  ".into(),
            project_id: None,
            source: None,
        }
    }
    async fn create(
        f: &Fixture,
        credential: Uuid,
        input: &PersonalInputBody,
    ) -> Result<Result<PersonalInputOutput, PersonalInputDbError>, sqlx::Error> {
        create_personal_input_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            input,
            Some("127.0.0.1"),
            "web",
        )
        .await
    }
    // Read actual identities, bodies, receipts, numbers and publication records,
    // not only counters. These observations run after the writer has finished.
    pub(crate) async fn snapshot(f: &Fixture) -> Vec<String> {
        let queries = [
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),title,content_json,number,version,deleted_at,hex(project_id)) v FROM documents ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),key,next_number,status,deleted_at,hex(root_document_id)) v FROM projects ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),title,content_json,number,version,hex(project_id),hex(status_id),deleted_at) v FROM tasks ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(workspace_id),hex(actor_user_id),hex(request_id),request_hash,intent,hex(document_id),hex(task_id),hex(project_id)) v FROM personal_input_commands ORDER BY workspace_id,actor_user_id,request_id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(task_id),hex(document_id),hex(request_id),request_hash,anchor) v FROM task_origins ORDER BY task_id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(task_id),hex(user_id)) v FROM task_assignees ORDER BY task_id,user_id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),hex(project_id)) v FROM workflows ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),hex(project_id),name,category,sort_key) v FROM statuses ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),hex(user_id),hex(group_id),role) v FROM project_members ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),seq,verb,hex(target_id),payload,channel) v FROM events ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),verb,hex(target_id),payload,ip) v FROM audit_log ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),hex(task_id),changes) v FROM task_activity ORDER BY id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(id),next_wiki_number) v FROM workspaces ORDER BY id)",
            "SELECT json_group_array(json_array(id,last_seq)) FROM event_sequence",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(document_id),hex(state),writer_generation,snapshot_cutoff_seq,tail_seq) v FROM document_states ORDER BY document_id)",
            "SELECT json_group_array(v) FROM (SELECT json_array(hex(task_id),hex(state),writer_generation,snapshot_cutoff_seq,tail_seq) v FROM task_states ORDER BY task_id)",
        ];
        let mut result = Vec::new();
        for query in queries {
            result.push(sqlx::query_scalar(query).fetch_one(&f.pool).await.unwrap());
        }
        assert!(sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&f.pool)
            .await
            .unwrap()
            .is_empty());
        result
    }
    fn identity(output: &PersonalInputOutput) -> Value {
        let mut value = serde_json::to_value(output).unwrap();
        value.as_object_mut().unwrap().remove("replayed");
        value
    }
    async fn project(f: &Fixture, credential: Uuid, key: &str) -> Uuid {
        crate::db::projects::create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            CreateProjectInput {
                key,
                name: "Personal",
                description: None,
                icon: None,
                visibility: "private",
                lead_user_id: None,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn note_quick_literal_receipts_and_response_loss_replay() {
        let (f, credential) = setup().await;
        for (number, intent) in [
            (2, PersonalInputIntent::Note),
            (3, PersonalInputIntent::Quick),
        ] {
            let body = input(intent);
            let result = create(&f, credential, &body).await.unwrap().unwrap();
            assert_eq!(result.document_display_id, format!("WIKI-{number}"));
            assert_eq!(result.task_id, None);
            assert_eq!(result.task_display_id, None);
            assert_eq!(result.project_id, None);
            assert!(!result.replayed);
            let id = Uuid::parse_str(&result.document_id).unwrap();
            let row:(String,String,i64,i64,Vec<u8>)=sqlx::query_as("SELECT title,content_json,schema_version,version,created_by FROM documents WHERE id=?1")
                .bind(id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(row.0, body.title.trim());
            assert_eq!(
                serde_json::from_str::<Value>(&row.1).unwrap(),
                crate::db::documents::empty_document_json()
            );
            assert_eq!(
                row.2,
                i64::from(crate::db::documents::DOCUMENT_SCHEMA_VERSION)
            );
            assert_eq!(row.3, 1);
            assert_eq!(row.4, f.user.as_bytes());
            let receipt:(String,String,Vec<u8>)=sqlx::query_as("SELECT request_hash,intent,document_id FROM personal_input_commands WHERE request_id=?1")
                .bind(body.request_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(
                receipt,
                (
                    request_hash(f.workspace, f.user, &body),
                    intent.as_str().into(),
                    id.as_bytes().to_vec()
                )
            );
            let publication: Vec<(String, String)> =
                sqlx::query_as("SELECT verb,payload FROM events WHERE target_id=?1 ORDER BY id")
                    .bind(id.as_bytes().as_slice())
                    .fetch_all(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(publication.len(), 1);
            assert_eq!(publication[0].0, "document.created");
            assert_eq!(
                serde_json::from_str::<Value>(&publication[0].1).unwrap()["documentId"],
                result.document_id
            );
            let after = snapshot(&f).await;
            let replay = create(&f, credential, &body).await.unwrap().unwrap();
            assert!(replay.replayed);
            assert_eq!(identity(&result), identity(&replay));
            assert_eq!(snapshot(&f).await, after);
            let mut changed = body.clone();
            changed.title = body.title.trim().into();
            assert!(matches!(
                create(&f, credential, &changed).await.unwrap(),
                Err(PersonalInputDbError::RequestMismatch)
            ));
            assert_eq!(snapshot(&f).await, after);
        }
        f.close().await;
    }

    #[tokio::test]
    async fn task_default_project_origin_assignment_and_native_identity() {
        let (f, credential) = setup().await;
        let body = input(PersonalInputIntent::Task);
        let result = create(&f, credential, &body).await.unwrap().unwrap();
        assert_eq!(result.document_display_id, "WIKI-2");
        assert_eq!(result.task_display_id.as_deref(), Some("INBOX-2"));
        let task = Uuid::parse_str(result.task_id.as_ref().unwrap()).unwrap();
        let document = Uuid::parse_str(&result.document_id).unwrap();
        let project = Uuid::parse_str(result.project_id.as_ref().unwrap()).unwrap();
        let row:(String,String,String,i64,i64,Vec<u8>,Vec<u8>)=sqlx::query_as("SELECT title,type,priority,schema_version,version,created_by,project_id FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            row,
            (
                body.title.trim().into(),
                "task".into(),
                "medium".into(),
                i64::from(crate::db::documents::DOCUMENT_SCHEMA_VERSION),
                1,
                f.user.as_bytes().to_vec(),
                project.as_bytes().to_vec()
            )
        );
        let assigned: Vec<Vec<u8>> =
            sqlx::query_scalar("SELECT user_id FROM task_assignees WHERE task_id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(assigned, vec![f.user.as_bytes().to_vec()]);
        let body_json: String = sqlx::query_scalar("SELECT content_json FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&body_json).unwrap(),
            crate::db::documents::empty_document_json()
        );
        let origin: (Vec<u8>, Vec<u8>, String, Option<String>) = sqlx::query_as(
            "SELECT document_id,request_id,request_hash,anchor FROM task_origins WHERE task_id=?1",
        )
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let normalized = json!({"title":body.title,"type":"task","priority":"medium","statusId":null,"startDate":null,"dueDate":null,"parentId":null,"milestoneId":null,"recurrence":null,"selfAssign":true});
        assert_eq!(
            origin,
            (
                document.as_bytes().to_vec(),
                body.request_id.as_bytes().to_vec(),
                origin_request_hash(f.user, project, None, &normalized),
                None
            )
        );
        let statuses: i64 = sqlx::query_scalar("SELECT count(*) FROM statuses WHERE project_id=?1")
            .bind(project.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(statuses, 6);
        let verbs: Vec<String> =
            sqlx::query_scalar("SELECT verb FROM events WHERE target_id=?1 ORDER BY id")
                .bind(task.as_bytes().as_slice())
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(verbs, vec!["task.created", "task.updated"]);
        let activities: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_activity WHERE task_id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert!(activities > 0);
        let after = snapshot(&f).await;
        let replay = create(&f, credential, &body).await.unwrap().unwrap();
        assert!(replay.replayed);
        assert_eq!(identity(&replay), identity(&result));
        assert_eq!(snapshot(&f).await, after);
        let next = create(&f, credential, &input(PersonalInputIntent::Task))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.project_id, result.project_id);
        assert_eq!(next.task_display_id.as_deref(), Some("INBOX-3"));
        assert_ne!(next.task_id, result.task_id);
        f.close().await;
    }

    #[tokio::test]
    async fn owner_credential_tenant_and_deleted_result_fail_closed() {
        let (f, credential) = setup().await;
        let body = input(PersonalInputIntent::Note);
        let result = create(&f, credential, &body).await.unwrap().unwrap();
        sqlx::query("UPDATE users SET personal_workspace_id=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        for request in [&body, &input(PersonalInputIntent::Note)] {
            assert!(matches!(
                create(&f, credential, request).await.unwrap(),
                Err(PersonalInputDbError::NotFound)
            ));
        }
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE users SET personal_workspace_id=?1 WHERE id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET expires_at=?2 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .bind(
                (crate::db::identity::stored_now()
                    + chrono::Duration::seconds(crate::auth::token::SESSION_TTL_SECS))
                .timestamp_micros(),
            )
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET suspended_at=1 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::Forbidden)
        ));
        sqlx::query("UPDATE users SET suspended_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_personal_input_backend(
                &f.backend,
                Uuid::now_v7(),
                f.user,
                credential,
                &body,
                None,
                "web"
            )
            .await
            .unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        let document = Uuid::parse_str(&result.document_id).unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let deleted = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        assert_eq!(snapshot(&f).await, deleted);
        sqlx::query("DELETE FROM documents WHERE id=?1")
            .bind(document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let purged = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        assert_eq!(snapshot(&f).await, purged);
        assert_eq!(
            sqlx::query_scalar::<_, Option<Vec<u8>>>(
                "SELECT document_id FROM personal_input_commands WHERE request_id=?1"
            )
            .bind(body.request_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            None
        );
        f.close().await;
    }

    #[tokio::test]
    async fn task_current_source_destination_and_replay_permissions() {
        let (f, credential) = setup().await;
        let destination = project(&f, credential, "DEST").await;
        let mut body = input(PersonalInputIntent::Task);
        body.project_id = Some(destination);
        body.source = Some(PersonalInputSource {
            document_id: f.document,
            anchor: Some("block-中".into()),
        });
        let created = create(&f, credential, &body).await.unwrap().unwrap();
        assert_eq!(created.document_id, f.document.to_string());
        assert_eq!(created.task_display_id.as_deref(), Some("DEST-2"));
        let task = Uuid::parse_str(created.task_id.as_ref().unwrap()).unwrap();
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(destination.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            create(&f, credential, &body)
                .await
                .unwrap()
                .unwrap()
                .replayed
        );
        let mut fresh = body.clone();
        fresh.request_id = Uuid::now_v7();
        let before = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &fresh).await.unwrap(),
            Err(PersonalInputDbError::Origin(TaskOriginDbError::Task(
                ProjectDbError::Archived
            )))
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE projects SET status='active' WHERE id=?1")
            .bind(destination.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE project_members SET role='viewer' WHERE project_id=?1 AND user_id=?2")
            .bind(destination.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(
            create(&f, credential, &body)
                .await
                .unwrap()
                .unwrap()
                .replayed
        ); // Receipt admission is current View, not fresh Edit.
        assert!(matches!(
            create(&f, credential, &fresh).await.unwrap(),
            Err(PersonalInputDbError::Origin(TaskOriginDbError::NotFound))
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("DELETE FROM project_members WHERE project_id=?1")
            .bind(destination.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(destination.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(
            create(&f, credential, &body)
                .await
                .unwrap()
                .unwrap()
                .replayed
        );
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        assert!(matches!(
            create(&f, credential, &fresh).await.unwrap(),
            Err(PersonalInputDbError::Origin(TaskOriginDbError::NotFound))
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET deleted_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        f.close().await;
    }

    #[tokio::test]
    async fn default_keys_archive_trash_collisions_and_exhaustion() {
        let (f, credential) = setup().await;
        let body = input(PersonalInputIntent::Task);
        let keys = default_project_keys(body.request_id);
        for (i, key) in keys.iter().enumerate() {
            let id = project(&f, credential, key).await;
            sqlx::query("UPDATE projects SET status='archived',deleted_at=?2 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .bind(if i % 2 == 0 { None } else { Some(1_i64) })
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let before = snapshot(&f).await;
        assert!(matches!(
            create(&f, credential, &body).await.unwrap(),
            Err(PersonalInputDbError::Project(ProjectDbError::Conflict))
        ));
        assert_eq!(snapshot(&f).await, before);
        let next = input(PersonalInputIntent::Task);
        let result = create(&f, credential, &next).await.unwrap().unwrap();
        let id = Uuid::parse_str(result.project_id.as_ref().unwrap()).unwrap();
        let key: String = sqlx::query_scalar("SELECT key FROM projects WHERE id=?1")
            .bind(id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(key, default_project_keys(next.request_id)[4]);
        let reused = create(&f, credential, &input(PersonalInputIntent::Task))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reused.project_id, result.project_id);
        f.close().await;
    }

    #[tokio::test]
    async fn staged_failures_and_deferred_commit_preserve_cause_then_explicit_retry() {
        let (f, credential) = setup().await;
        let body = input(PersonalInputIntent::Task);
        let before = snapshot(&f).await;
        sqlx::query(
            "CREATE TABLE personal_input_fk_probe(id BLOB REFERENCES documents(id)) STRICT",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        for table in [
            "events",
            "audit_log",
            "task_activity",
            "task_assignees",
            "task_origins",
            "personal_input_commands",
        ] {
            sqlx::query(&format!("CREATE TRIGGER personal_input_refuse AFTER INSERT ON {table} BEGIN INSERT INTO personal_input_fk_probe(id) VALUES(zeroblob(16)); END")).execute(&f.pool).await.unwrap();
            let error = create(&f, credential, &body).await.unwrap_err();
            assert!(
                error
                    .as_database_error()
                    .is_some_and(|error| error.is_foreign_key_violation()),
                "original failure retained: {error}"
            );
            assert_eq!(snapshot(&f).await, before);
            sqlx::query("DROP TRIGGER personal_input_refuse")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("CREATE TABLE personal_input_deferred_probe(id BLOB REFERENCES documents(id) DEFERRABLE INITIALLY DEFERRED) STRICT").execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER personal_input_commit_refuse AFTER INSERT ON personal_input_commands BEGIN INSERT INTO personal_input_deferred_probe(id) VALUES(zeroblob(16)); END").execute(&f.pool).await.unwrap();
        let error = create(&f, credential, &body).await.unwrap_err();
        let sqlx::Error::AnyDriverError(inner) = &error else {
            panic!("typed commit uncertainty required: {error}")
        };
        let unknown = inner
            .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
            .unwrap();
        assert_eq!(
            unknown.settlement,
            crate::db::backend::CommitSettlement::LocalWriterReconcile
        );
        assert!(unknown
            .source
            .source
            .as_database_error()
            .is_some_and(|error| error.is_foreign_key_violation()));
        // Local queued rollback observation only; never claim remote settlement.
        assert_eq!(snapshot(&f).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM personal_input_deferred_probe")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER personal_input_commit_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = create(&f, credential, &body).await.unwrap().unwrap();
        assert_eq!(healthy.document_display_id, "WIKI-2");
        assert_eq!(healthy.task_display_id.as_deref(), Some("INBOX-2"));
        assert!(!healthy.replayed);
        assert!(
            create(&f, credential, &body)
                .await
                .unwrap()
                .unwrap()
                .replayed
        );
        f.close().await;
    }

    #[tokio::test]
    async fn waiting_writer_rechecks_revocation_before_any_publication() {
        let (mut f, credential) = setup().await;
        f.pool.close().await;
        f.pool = crate::db::pool::connect_sqlite_app(&f.path, 3)
            .await
            .unwrap();
        f.backend = Backend::Sqlite(f.pool.clone());
        let body = input(PersonalInputIntent::Task);
        let before = snapshot(&f).await;
        let mut revoker = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(family) = revoker.operation() else {
            unreachable!()
        };
        family
            .execute(
                "UPDATE users SET personal_workspace_id=NULL WHERE id=?1",
                &[Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        let backend = f.backend.clone();
        let workspace = f.workspace;
        let actor = f.user;
        let (started, wait) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            started.send(()).unwrap();
            create_personal_input_backend(
                &backend, workspace, actor, credential, &body, None, "web",
            )
            .await
        });
        wait.await.unwrap();
        revoker.commit().await.unwrap();
        assert!(matches!(
            pending.await.unwrap().unwrap(),
            Err(PersonalInputDbError::NotFound)
        ));
        assert_eq!(snapshot(&f).await, before);
        sqlx::query("UPDATE users SET personal_workspace_id=?1 WHERE id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(create(&f, credential, &input(PersonalInputIntent::Task))
            .await
            .unwrap()
            .is_ok());
        f.close().await;
    }

    #[tokio::test]
    async fn concurrent_same_command_has_one_identity_and_one_publication() {
        let (mut f, credential) = setup().await;
        f.pool.close().await;
        f.pool = crate::db::pool::connect_sqlite_app(&f.path, 3)
            .await
            .unwrap();
        f.backend = Backend::Sqlite(f.pool.clone());
        let request = input(PersonalInputIntent::Task);
        let (left, right) = tokio::join!(
            create(&f, credential, &request),
            create(&f, credential, &request)
        );
        let left = left.unwrap().unwrap();
        let right = right.unwrap().unwrap();
        assert_ne!(left.replayed, right.replayed);
        assert_eq!(identity(&left), identity(&right));
        assert_eq!(left.document_display_id, "WIKI-2");
        assert_eq!(left.task_display_id.as_deref(), Some("INBOX-2"));
        let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM personal_input_commands),(SELECT count(*) FROM task_origins),(SELECT count(*) FROM task_assignees),(SELECT count(*) FROM events WHERE verb='task.created')").fetch_one(&f.pool).await.unwrap();
        assert_eq!(counts, (1, 1, 1, 1));
        let after = snapshot(&f).await;
        let replay = create(&f, credential, &request).await.unwrap().unwrap();
        assert!(replay.replayed);
        assert_eq!(identity(&replay), identity(&left));
        assert_eq!(snapshot(&f).await, after);
        f.close().await;
    }
}
