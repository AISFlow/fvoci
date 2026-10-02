//! Atomic personal input into ordinary entities, with an append-only retry receipt.
use crate::api::personal_input_dto::{PersonalInputBody, PersonalInputIntent, PersonalInputOutput};
use crate::db::context::{lock_membership_users, lock_tree, recheck_session, set_tenant};
use crate::db::documents::{create_wiki_document_tx, CreateDocumentInput};
use crate::db::projects::{create_project_tx, CreateProjectInput, ProjectDbError};
use crate::db::task_origins::{
    create_document_task_tx, document_view_permission, origin_request_hash, task_view_permission,
    DocumentTaskRequest, TaskOriginDbError,
};
use crate::db::tasks::CreateTaskInput;
use crate::projects::ProjectPermission;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

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
                let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM fvoci.projects WHERE workspace_id = $1 AND key = 'INBOX' AND deleted_at IS NULL AND status <> 'archived'")
                    .bind(workspace_id).fetch_optional(&mut **tx).await?;
                Some(match existing {
                    Some(id) => id,
                    None => match create_project_tx(
                        tx,
                        workspace_id,
                        actor,
                        session_id,
                        CreateProjectInput {
                            key: "INBOX",
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
                    },
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

#[cfg(test)]
mod tests {
    use super::*;
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
