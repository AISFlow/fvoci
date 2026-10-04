//! Atomic database outbox consumer that fans events into per-user in-app notifications.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::Row;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::codec::Cell;
use crate::db::notifications::{find_prefs_tx, resolved_store_prefs, NotificationInsert};
use crate::db::outbox::{
    advance_cursor_backend_tx, mark_processed_backend_tx, BackendOutboxEvent, OutboxEvent,
};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::projects::ProjectPermission;

pub const NOTIFICATIONS_CONSUMER: &str = "notifications";

pub struct NotificationsConsumer;

impl OutboxConsumer for NotificationsConsumer {
    fn name(&self) -> &str {
        NOTIFICATIONS_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::DatabaseAtomic
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { process_notification_event(pool, lease_owner, event).await })
    }
    fn deliver_backend<'a>(
        &'a self,
        backend: &'a Backend,
        lease_owner: Uuid,
        event: &'a BackendOutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(
            async move { process_notification_event_backend(backend, lease_owner, event).await },
        )
    }
}

pub fn notifications_consumer() -> Arc<dyn OutboxConsumer> {
    Arc::new(NotificationsConsumer)
}

pub async fn process_notification_event(
    pool: &PgPool,
    lease_owner: Uuid,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    process_notification_event_backend(
        &Backend::Postgres(pool.clone()),
        lease_owner,
        &event.clone().into(),
    )
    .await
}

pub async fn process_notification_event_backend(
    backend: &Backend,
    lease_owner: Uuid,
    event: &BackendOutboxEvent,
) -> Result<(), OutboxProcessError> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    if let Some(workspace) = event.workspace_id {
        tx.operation().set_tenant(workspace).await?;
    }
    if mark_processed_backend_tx(&mut tx, NOTIFICATIONS_CONSUMER, event.id).await? {
        let rows = notify_for_event_backend(&mut tx.operation(), event).await?;
        tx.operation().insert_notifications(&rows).await?;
    }
    if !advance_cursor_backend_tx(
        &mut tx,
        NOTIFICATIONS_CONSUMER,
        lease_owner,
        &event.cursor(),
    )
    .await?
    {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in notification transaction".into(),
        ));
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| OutboxProcessError::Db(sqlx::Error::AnyDriverError(Box::new(e))))?;
    Ok(())
}

fn as_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty())
}

fn as_uuid(value: &Value) -> Option<Uuid> {
    as_string(value).and_then(|s| Uuid::parse_str(s).ok())
}

fn payload_string<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(as_string)
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload.get(key).and_then(as_uuid)
}

fn payload_uuid_array(payload: &Value, key: &str) -> Vec<Uuid> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(as_uuid).collect())
        .unwrap_or_default()
}

async fn user_is_present_pg(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let present: Option<(bool,)> =
        sqlx::query_as("SELECT true FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(present.is_some())
}

async fn recipients_for(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    candidates: impl IntoIterator<Item = Uuid>,
    actor_user_id: Option<Uuid>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut unique = BTreeSet::new();
    for id in candidates {
        if Some(id) == actor_user_id {
            continue;
        }
        unique.insert(id);
    }
    let mut out = Vec::new();
    for user_id in unique {
        if tx
            .membership_role(workspace_id, user_id, false)
            .await?
            .is_none()
        {
            continue;
        }
        if !user_is_present(tx, user_id).await? {
            continue;
        }
        out.push(user_id);
    }
    Ok(out)
}

async fn can_view_project(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    user: Uuid,
    project: Uuid,
) -> Result<bool, sqlx::Error> {
    Ok(tx
        .project_permission_by_id(workspace, user, project)
        .await?
        .is_some_and(|permission| permission.at_least(ProjectPermission::View)))
}

async fn can_view_document(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    user: Uuid,
    document: Uuid,
) -> Result<bool, sqlx::Error> {
    let project: Option<Option<Uuid>> = match tx {
        OperationTx::Postgres(tx) => sqlx::query_scalar(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
        ).bind(workspace).bind(document).fetch_optional(&mut ***tx).await?,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query("SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                &[Cell::uuid(workspace), Cell::uuid(document)]).await?
                .first().map(|row| row.cell(0)?.optional(Cell::id)).transpose()?
        }
    };
    let Some(project) = project else {
        return Ok(false);
    };
    if let Some(project) = project {
        return can_view_project(tx, workspace, user, project).await;
    }
    Ok(tx
        .document_permission(workspace, user, document, true)
        .await?
        .at_least(ProjectPermission::View))
}

async fn recipients_who_can_view_project(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    user_ids: Vec<Uuid>,
    project_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut visible = Vec::new();
    for user_id in user_ids {
        if can_view_project(tx, workspace_id, user_id, project_id).await? {
            visible.push(user_id);
        }
    }
    Ok(visible)
}

async fn list_task_assignees_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY user_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

struct TaskSnap {
    number: i32,
    title: String,
    project_id: Uuid,
}

async fn task_snapshot_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskSnap>, sqlx::Error> {
    let row: Option<(i32, String, Uuid)> = sqlx::query_as(
        r#"
        SELECT number, title, project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(number, title, project_id)| TaskSnap {
        number,
        title,
        project_id,
    }))
}

fn base_insert(
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
    user_id: Uuid,
    payload: Value,
) -> NotificationInsert {
    NotificationInsert {
        id: Uuid::now_v7(),
        workspace_id,
        user_id,
        event_id: event.id,
        verb: event.verb.clone(),
        actor_user_id: event.actor_user_id,
        target_type: event.target_type.clone(),
        target_id: event.target_id,
        payload,
    }
}

async fn keep_by_prefs(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    rows: Vec<NotificationInsert>,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let mut kept = Vec::new();
    for row in rows {
        let prefs = resolved_store_prefs(tx.notification_prefs(workspace_id, row.user_id).await?);
        if prefs.in_app || prefs.mail_digest {
            kept.push(row);
        }
    }
    Ok(kept)
}

async fn task_created(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(task_id) = payload_uuid(&event.payload, "taskId") else {
        return Ok(Vec::new());
    };
    let Some(snap) = task_snapshot(tx, workspace_id, task_id).await? else {
        return Ok(Vec::new());
    };
    let assignees = list_task_assignees(tx, workspace_id, task_id).await?;
    let members = recipients_for(tx, workspace_id, assignees, event.actor_user_id).await?;
    let recipients =
        recipients_who_can_view_project(tx, workspace_id, members, snap.project_id).await?;
    let payload = json!({
        "number": snap.number,
        "title": snap.title,
        "projectId": snap.project_id.to_string(),
    });
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
        .collect())
}

async fn task_updated(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(task_id) = payload_uuid(&event.payload, "taskId") else {
        return Ok(Vec::new());
    };
    let from = payload_string(&event.payload, "from");
    let to = payload_string(&event.payload, "to");
    if let (Some(from), Some(to)) = (from, to) {
        let Some(snap) = task_snapshot(tx, workspace_id, task_id).await? else {
            return Ok(Vec::new());
        };
        let assignees = list_task_assignees(tx, workspace_id, task_id).await?;
        let members = recipients_for(tx, workspace_id, assignees, event.actor_user_id).await?;
        let recipients =
            recipients_who_can_view_project(tx, workspace_id, members, snap.project_id).await?;
        let from_name = status_name(tx, workspace_id, from).await?;
        let to_name = status_name(tx, workspace_id, to).await?;
        let payload = json!({
            "number": snap.number,
            "title": snap.title,
            "projectId": snap.project_id.to_string(),
            "fromName": from_name,
            "toName": to_name,
        });
        return Ok(recipients
            .into_iter()
            .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
            .collect());
    }
    if event
        .payload
        .get("assigneeIds")
        .and_then(Value::as_array)
        .is_some()
    {
        let added = payload_uuid_array(&event.payload, "addedAssigneeIds");
        if added.is_empty() {
            return Ok(Vec::new());
        }
        let Some(snap) = task_snapshot(tx, workspace_id, task_id).await? else {
            return Ok(Vec::new());
        };
        let members = recipients_for(tx, workspace_id, added, event.actor_user_id).await?;
        let recipients =
            recipients_who_can_view_project(tx, workspace_id, members, snap.project_id).await?;
        let payload = json!({
            "number": snap.number,
            "title": snap.title,
            "projectId": snap.project_id.to_string(),
        });
        return Ok(recipients
            .into_iter()
            .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
            .collect());
    }
    Ok(Vec::new())
}

async fn status_name_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: &str,
) -> Result<String, sqlx::Error> {
    let Ok(id) = Uuid::parse_str(status_id) else {
        return Ok(status_id.to_string());
    };
    let row: Option<(String,)> =
        sqlx::query_as("SELECT name FROM fvoci.statuses WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row
        .map(|(name,)| name)
        .unwrap_or_else(|| status_id.to_string()))
}

async fn task_deleted(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let candidates = payload_uuid_array(&event.payload, "assigneeIds");
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let recipients = recipients_for(tx, workspace_id, candidates, event.actor_user_id).await?;
    let mut payload = serde_json::Map::new();
    if let Some(number) = event.payload.get("number").and_then(Value::as_i64) {
        payload.insert("number".into(), json!(number));
    }
    if let Some(project_id) = payload_string(&event.payload, "projectId") {
        payload.insert("projectId".into(), json!(project_id));
    }
    let payload = Value::Object(payload);
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
        .collect())
}

async fn project_member_changed(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(target_user_id) = payload_uuid(&event.payload, "userId") else {
        return Ok(Vec::new());
    };
    let Some(project_id) = payload_uuid(&event.payload, "projectId") else {
        return Ok(Vec::new());
    };
    let Some(project_name) = notification_project_name(tx, workspace_id, project_id).await? else {
        return Ok(Vec::new());
    };
    let recipients =
        recipients_for(tx, workspace_id, [target_user_id], event.actor_user_id).await?;
    let mut payload = json!({
        "projectId": project_id.to_string(),
        "projectName": project_name,
    });
    if let Some(role) = payload_string(&event.payload, "role") {
        payload["role"] = json!(role);
    }
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
        .collect())
}

fn comment_notify_payload(
    comment_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    parent_id: Option<Uuid>,
) -> Value {
    let mut payload = json!({ "commentId": comment_id.to_string(), "parentId": Value::Null });
    if let Some(document_id) = document_id {
        payload["documentId"] = json!(document_id.to_string());
    }
    if let Some(task_id) = task_id {
        payload["taskId"] = json!(task_id.to_string());
    }
    payload["parentId"] = match parent_id {
        Some(id) => json!(id.to_string()),
        None => Value::Null,
    };
    payload
}

async fn load_comment_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<CommentSnap>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, document_id, task_id, parent_id, created_by, body
        FROM fvoci.comments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(comment_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| CommentSnap {
        id: row.get("id"),
        document_id: row.get("document_id"),
        task_id: row.get("task_id"),
        parent_id: row.get("parent_id"),
        created_by: row.get("created_by"),
        body: row.get("body"),
    }))
}

struct CommentSnap {
    id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    created_by: Uuid,
    body: String,
}

async fn comment_created_audience(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
    comment: &CommentSnap,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut candidates = payload_uuid_array(&event.payload, "mentionedUserIds");
    for group_id in payload_uuid_array(&event.payload, "mentionedGroupIds") {
        candidates.extend(notification_group_members(tx, workspace_id, group_id).await?);
    }
    if let Some(parent_id) = comment.parent_id {
        if let Some(parent) = load_comment(tx, workspace_id, parent_id).await? {
            candidates.push(parent.created_by);
        }
    }
    if let Some(task_id) = comment.task_id {
        candidates.extend(list_task_assignees(tx, workspace_id, task_id).await?);
    }
    if let Some(document_id) = comment.document_id {
        if let Some(created_by) =
            notification_document_creator(tx, workspace_id, document_id).await?
        {
            candidates.push(created_by);
        }
    }
    let members = recipients_for(tx, workspace_id, candidates, event.actor_user_id).await?;
    if let Some(task_id) = comment.task_id {
        let task = task_snapshot(tx, workspace_id, task_id).await?;
        let Some(task) = task else {
            return Ok(Vec::new());
        };
        return recipients_who_can_view_project(tx, workspace_id, members, task.project_id).await;
    }
    if let Some(document_id) = comment.document_id {
        let mut visible = Vec::new();
        for user_id in members {
            if can_view_document(tx, workspace_id, user_id, document_id).await? {
                visible.push(user_id);
            }
        }
        return Ok(visible);
    }
    Ok(members)
}

async fn comment_created(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(comment_id) = payload_uuid(&event.payload, "commentId") else {
        return Ok(Vec::new());
    };
    let Some(comment) = load_comment(tx, workspace_id, comment_id).await? else {
        return Ok(Vec::new());
    };
    let recipients = comment_created_audience(tx, event, workspace_id, &comment).await?;
    let payload = comment_notify_payload(
        comment.id,
        comment.document_id,
        comment.task_id,
        comment.parent_id,
    );
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
        .collect())
}

async fn comment_resolved(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(comment_id) = payload_uuid(&event.payload, "commentId") else {
        return Ok(Vec::new());
    };
    let Some(comment) = load_comment(tx, workspace_id, comment_id).await? else {
        return Ok(Vec::new());
    };
    let mut candidates = vec![comment.created_by];
    if let Some(task_id) = comment.task_id {
        candidates.extend(list_task_assignees(tx, workspace_id, task_id).await?);
    }
    let recipients = recipients_for(tx, workspace_id, candidates, event.actor_user_id).await?;
    let payload = comment_notify_payload(
        comment.id,
        comment.document_id,
        comment.task_id,
        comment.parent_id,
    );
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, payload.clone()))
        .collect())
}

async fn invitation_accepted(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(invitation_id) = event.target_id else {
        return Ok(Vec::new());
    };
    let Some(invited_by) = notification_inviter(tx, workspace_id, invitation_id).await? else {
        return Ok(Vec::new());
    };
    let recipients = recipients_for(tx, workspace_id, [invited_by], event.actor_user_id).await?;
    Ok(recipients
        .into_iter()
        .map(|user_id| base_insert(event, workspace_id, user_id, json!({})))
        .collect())
}

pub async fn notify_for_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    notify_for_event_backend(&mut OperationTx::Postgres(tx), &event.clone().into()).await
}

pub async fn notify_for_event_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(workspace_id) = event.workspace_id else {
        return Ok(Vec::new());
    };
    if !tx.workspace_is_live(workspace_id).await? {
        return Ok(Vec::new());
    }
    let rows = match event.verb.as_str() {
        "task.created" => task_created(tx, event, workspace_id).await?,
        "task.updated" => task_updated(tx, event, workspace_id).await?,
        "task.deleted" => task_deleted(tx, event, workspace_id).await?,
        "project_member.added" | "project_member.role_changed" => {
            project_member_changed(tx, event, workspace_id).await?
        }
        "invitation.accepted" => invitation_accepted(tx, event, workspace_id).await?,
        "comment.created" => comment_created(tx, event, workspace_id).await?,
        "comment.resolved" => comment_resolved(tx, event, workspace_id).await?,
        _ => Vec::new(),
    };
    keep_by_prefs(tx, workspace_id, rows).await
}

#[derive(Debug, Clone)]
pub struct OutboundMail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

fn format_person_name_ko(given_name: &str, family_name: Option<&str>) -> String {
    match family_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(family) => format!("{family}{given_name}"),
        None => given_name.to_string(),
    }
}

/// Immediate comment mail: source `listImmediateCommentMails`. Default
/// `mailImmediate` is true when the recipient has no prefs row.
pub async fn list_immediate_comment_mails(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<Vec<OutboundMail>, sqlx::Error> {
    if event.verb != "comment.created" {
        return Ok(Vec::new());
    }
    let Some(workspace_id) = event.workspace_id else {
        return Ok(Vec::new());
    };
    let Some(comment_id) = payload_uuid(&event.payload, "commentId") else {
        return Ok(Vec::new());
    };
    let Some(comment) = load_comment_pg(tx, workspace_id, comment_id).await? else {
        return Ok(Vec::new());
    };
    let recipients = comment_created_audience(
        &mut OperationTx::Postgres(tx),
        &event.clone().into(),
        workspace_id,
        &comment,
    )
    .await?;
    let actor_name = if let Some(actor_id) = event.actor_user_id {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT given_name, family_name FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(actor_id)
        .fetch_optional(&mut **tx)
        .await?;
        row.map(|(given, family)| format_person_name_ko(&given, family.as_deref()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let subject = crate::mail::templates::COMMENT_SUBJECT;
    let text = crate::mail::templates::comment_text(&actor_name, &comment.body);
    let mut mails = Vec::new();
    for user_id in recipients {
        let prefs = resolved_store_prefs(find_prefs_tx(tx, workspace_id, user_id).await?);
        if !prefs.mail_immediate {
            continue;
        }
        let user: Option<(String,)> =
            sqlx::query_as("SELECT email FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
                .bind(user_id)
                .fetch_optional(&mut **tx)
                .await?;
        let Some((email,)) = user else {
            continue;
        };
        mails.push(OutboundMail {
            to: email,
            subject: subject.to_string(),
            text: text.clone(),
        });
    }
    Ok(mails)
}

pub async fn identity_mail_for_event(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<Option<OutboundMail>, sqlx::Error> {
    use crate::settings::messages::Message;
    let (subject, text) = match event.verb.as_str() {
        "identity.linked" => (Message::IdentityLinkedSubject, Message::IdentityLinkedText),
        "identity.unlinked" => (
            Message::IdentityUnlinkedSubject,
            Message::IdentityUnlinkedText,
        ),
        _ => return Ok(None),
    };
    let Some(actor_id) = event.actor_user_id else {
        return Ok(None);
    };
    let Some(provider) = event.payload.get("provider").and_then(Value::as_str) else {
        return Ok(None);
    };
    let user: Option<(String,)> =
        sqlx::query_as("SELECT email FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
            .bind(actor_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((email,)) = user else {
        return Ok(None);
    };
    // Read at delivery time (a redelivery uses the then-current copy); the
    // caller commits this transaction before SMTP.
    let messages = crate::settings::messages::load(&mut **tx).await?;
    Ok(Some(OutboundMail {
        to: email,
        subject: messages.subject(subject),
        text: messages.render(text, &[("provider", provider)]),
    }))
}

async fn user_is_present(tx: &mut OperationTx<'_, '_>, user: Uuid) -> Result<bool, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => user_is_present_pg(tx, user).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT 1 FROM users WHERE id=?1 AND deleted_at IS NULL",
                    &[Cell::uuid(user)],
                )
                .await?;
            Ok(!rows.is_empty())
        }
    }
}
async fn list_task_assignees(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    task: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => list_task_assignees_pg(tx, workspace, task).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query("SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 ORDER BY user_id",&[Cell::uuid(workspace),Cell::uuid(task)]).await?.iter().map(|r|r.cell(0)?.id()).collect()
        }
    }
}
async fn task_snapshot(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    task: Uuid,
) -> Result<Option<TaskSnap>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => task_snapshot_pg(tx, workspace, task).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows=tx.query("SELECT number,title,project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(task)]).await?;
            rows.first()
                .map(|r| {
                    Ok(TaskSnap {
                        number: i32::try_from(r.cell(0)?.integer()?).map_err(|_| {
                            sqlx::Error::Protocol("notification task number out of range".into())
                        })?,
                        title: r.cell(1)?.string()?,
                        project_id: r.cell(2)?.id()?,
                    })
                })
                .transpose()
        }
    }
}
async fn status_name(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    status: &str,
) -> Result<String, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => status_name_pg(tx, workspace, status).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let Ok(id) = Uuid::parse_str(status) else {
                return Ok(status.to_owned());
            };
            let rows = tx
                .query(
                    "SELECT name FROM statuses WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(id)],
                )
                .await?;
            rows.first()
                .map(|r| r.cell(0)?.string())
                .unwrap_or_else(|| Ok(status.to_owned()))
        }
    }
}
async fn load_comment(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    comment: Uuid,
) -> Result<Option<CommentSnap>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => load_comment_pg(tx, workspace, comment).await,
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows=tx.query("SELECT id,document_id,task_id,parent_id,created_by,body FROM comments WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(comment)]).await?;
            rows.first()
                .map(|r| {
                    Ok(CommentSnap {
                        id: r.cell(0)?.id()?,
                        document_id: r.cell(1)?.optional(Cell::id)?,
                        task_id: r.cell(2)?.optional(Cell::id)?,
                        parent_id: r.cell(3)?.optional(Cell::id)?,
                        created_by: r.cell(4)?.id()?,
                        body: r.cell(5)?.string()?,
                    })
                })
                .transpose()
        }
    }
}
async fn notification_project_name(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    project: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx)=>sqlx::query_scalar("SELECT name FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL").bind(workspace).bind(project).fetch_optional(&mut ***tx).await,
        OperationTx::SqliteFamily(tx)=>{
            tx.require_tenant(workspace)?;tx.require_system_context()?;
            let rows=tx.query("SELECT name FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(project)]).await?;
            rows.first().map(|r|r.cell(0)?.string()).transpose()
        }
    }
}
async fn notification_group_members(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    group: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT user_id FROM fvoci.group_members WHERE workspace_id=$1 AND group_id=$2",
            )
            .bind(workspace)
            .bind(group)
            .fetch_all(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            tx.query(
                "SELECT user_id FROM group_members WHERE workspace_id=?1 AND group_id=?2",
                &[Cell::uuid(workspace), Cell::uuid(group)],
            )
            .await?
            .iter()
            .map(|r| r.cell(0)?.id())
            .collect()
        }
    }
}
async fn notification_document_creator(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    document: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT created_by FROM fvoci.documents WHERE workspace_id=$1 AND id=$2",
            )
            .bind(workspace)
            .bind(document)
            .fetch_optional(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT created_by FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(document)],
                )
                .await?;
            rows.first().map(|r| r.cell(0)?.id()).transpose()
        }
    }
}
async fn notification_inviter(
    tx: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    invitation: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match tx {
        OperationTx::Postgres(tx) => {
            sqlx::query_scalar(
                "SELECT invited_by FROM fvoci.invitations WHERE workspace_id=$1 AND id=$2",
            )
            .bind(workspace)
            .bind(invitation)
            .fetch_optional(&mut ***tx)
            .await
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_tenant(workspace)?;
            tx.require_system_context()?;
            let rows = tx
                .query(
                    "SELECT invited_by FROM invitations WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(invitation)],
                )
                .await?;
            rows.first().map(|r| r.cell(0)?.id()).transpose()
        }
    }
}

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::db::outbox::{
        ensure_consumer_backend, fetch_cursor_backend, lease_consumer_backend, OutboxCursor,
    };

    async fn effect_counts(f: &Fixture, event: Uuid) -> (i64, i64) {
        let notifications =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE event_id=?1")
                .bind(event.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let processed = sqlx::query_scalar(
            "SELECT COUNT(*) FROM processed_events WHERE consumer=?1 AND event_id=?2",
        )
        .bind(NOTIFICATIONS_CONSUMER)
        .bind(event.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        (notifications, processed)
    }

    #[tokio::test]
    async fn actual_backend_consumer_effect_marker_cursor_rollback_duplicate_and_current_target() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        ensure_consumer_backend(&f.backend, NOTIFICATIONS_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(
            lease_consumer_backend(&f.backend, NOTIFICATIONS_CONSUMER, owner, 60)
                .await
                .unwrap()
        );
        let event = f.append_comment_event("comment.created").await;
        assert!(
            process_notification_event_backend(&f.backend, Uuid::now_v7(), &event)
                .await
                .is_err()
        );
        assert_eq!(
            effect_counts(&f, event.id).await,
            (0, 0),
            "false cursor advance must roll back both effect and marker"
        );
        assert!(matches!(
            fetch_cursor_backend(&f.backend, NOTIFICATIONS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        ));
        process_notification_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(effect_counts(&f, event.id).await, (1, 1));
        assert!(
            process_notification_event_backend(&f.backend, owner, &event)
                .await
                .is_err(),
            "an already advanced cursor is not a fresh successful advance"
        );
        assert_eq!(
            effect_counts(&f, event.id).await,
            (1, 1),
            "duplicate event must not duplicate recipient effect"
        );
        assert!(
            matches!(fetch_cursor_backend(&f.backend,NOTIFICATIONS_CONSUMER).await.unwrap(),Some(OutboxCursor::SqliteFamily{seq}) if seq==event.seq)
        );
        sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let hidden = f.append_comment_event("comment.created").await;
        process_notification_event_backend(&f.backend, owner, &hidden)
            .await
            .unwrap();
        assert_eq!(
            effect_counts(&f, hidden.id).await,
            (0, 1),
            "current target policy must filter a formerly authorized guest"
        );
        assert!(
            matches!(fetch_cursor_backend(&f.backend,NOTIFICATIONS_CONSUMER).await.unwrap(),Some(OutboxCursor::SqliteFamily{seq}) if seq==hidden.seq)
        );
        f.finish().await;
    }

    // This test is deliberately a regression expectation, not a replacement
    // policy: both verbs must reject the same guest after the same grant loss.
    #[tokio::test]
    async fn actual_backend_resolved_comment_obeys_current_target_like_created() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let created = f.append_comment_event("comment.created").await;
        let resolved = f.append_comment_event("comment.resolved").await;
        for event in [&created, &resolved] {
            let mut tx = f.backend.begin_write().await.unwrap();
            tx.operation().set_tenant(f.workspace).await.unwrap();
            let previous = tx.operation().set_system().await.unwrap();
            let rows = notify_for_event_backend(&mut tx.operation(), event)
                .await
                .unwrap();
            tx.operation().restore_system(previous).await.unwrap();
            tx.rollback().await.unwrap();
            assert!(
                rows.is_empty(),
                "{} exposed a comment target after grant removal",
                event.verb
            );
        }
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_cancelled_effect_rolls_back_before_single_connection_reuse() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        ensure_consumer_backend(&f.backend, NOTIFICATIONS_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(
            lease_consumer_backend(&f.backend, NOTIFICATIONS_CONSUMER, owner, 60)
                .await
                .unwrap()
        );
        let event = f.append_comment_event("comment.created").await;
        let backend = f.backend.clone();
        let pending_event = event.clone();
        let workspace = f.workspace;
        let (ready, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation().set_tenant(workspace).await.unwrap();
            tx.operation().set_system().await.unwrap();
            let rows = notify_for_event_backend(&mut tx.operation(), &pending_event)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            tx.operation().insert_notifications(&rows).await.unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            tx.rollback().await.unwrap();
        });
        received.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(effect_counts(&f, event.id).await, (0, 0));
        process_notification_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(effect_counts(&f, event.id).await, (1, 1));
        let mut tx = f.backend.begin_read().await.unwrap();
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            panic!("local fixture")
        };
        assert!(family.tenant().is_none());
        assert!(family.require_system_context().is_err());
        tx.rollback().await.unwrap();
        f.finish().await;
    }
}
