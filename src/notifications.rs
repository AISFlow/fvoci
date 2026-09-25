//! PgOnly outbox consumer that fans events into per-user in-app notifications.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::Row;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::db::documents::document_permission;
use crate::db::notifications::{
    find_prefs_tx, insert_many, resolved_store_prefs, NotificationInsert,
};
use crate::db::outbox::{advance_cursor_tx, mark_processed_tx, OutboxEvent};
use crate::db::projects::project_member_role;
use crate::db::workspace::{membership_role, WorkspaceRole};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::projects::{effective_permission, ProjectPermission};

pub const NOTIFICATIONS_CONSUMER: &str = "notifications";

pub struct NotificationsConsumer;

impl OutboxConsumer for NotificationsConsumer {
    fn name(&self) -> &str {
        NOTIFICATIONS_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { process_notification_event(pool, lease_owner, event).await })
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
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    if let Some(workspace_id) = event.workspace_id {
        set_tenant(&mut tx, workspace_id).await?;
    }
    let fresh = mark_processed_tx(&mut tx, NOTIFICATIONS_CONSUMER, event.id).await?;
    if fresh {
        let rows = notify_for_event(&mut tx, event).await?;
        insert_many(&mut tx, &rows).await?;
    }
    if !advance_cursor_tx(
        &mut tx,
        NOTIFICATIONS_CONSUMER,
        lease_owner,
        &event.xact,
        event.seq,
    )
    .await?
    {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in pg-only tx".into(),
        ));
    }
    tx.commit().await?;
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

async fn user_is_present(
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
    tx: &mut Transaction<'_, Postgres>,
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
        if membership_role(tx, workspace_id, user_id).await?.is_none() {
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
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let visibility: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT visibility
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((visibility,)) = visibility else {
        return Ok(false);
    };
    let workspace_role = membership_role(tx, workspace_id, user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest);
    let member_role = project_member_role(tx, workspace_id, project_id, user_id).await?;
    Ok(
        effective_permission(workspace_role, &visibility, member_role)
            .at_least(ProjectPermission::View),
    )
}

async fn can_view_document(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id,)) = row else {
        return Ok(false);
    };
    if let Some(project_id) = project_id {
        return can_view_project(tx, workspace_id, user_id, project_id).await;
    }
    Ok(
        document_permission(tx, workspace_id, user_id, document_id, true)
            .await?
            .at_least(ProjectPermission::View),
    )
}

async fn recipients_who_can_view_project(
    tx: &mut Transaction<'_, Postgres>,
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

async fn list_task_assignees(
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

async fn task_snapshot(
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
    event: &OutboxEvent,
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
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    rows: Vec<NotificationInsert>,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let mut kept = Vec::new();
    for row in rows {
        let prefs = resolved_store_prefs(find_prefs_tx(tx, workspace_id, row.user_id).await?);
        if prefs.in_app || prefs.mail_digest {
            kept.push(row);
        }
    }
    Ok(kept)
}

async fn task_created(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
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

async fn status_name(
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(target_user_id) = payload_uuid(&event.payload, "userId") else {
        return Ok(Vec::new());
    };
    let Some(project_id) = payload_uuid(&event.payload, "projectId") else {
        return Ok(Vec::new());
    };
    let project: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT name FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_name,)) = project else {
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

async fn load_comment(
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
    workspace_id: Uuid,
    comment: &CommentSnap,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut candidates = payload_uuid_array(&event.payload, "mentionedUserIds");
    for group_id in payload_uuid_array(&event.payload, "mentionedGroupIds") {
        let members: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT user_id
            FROM fvoci.group_members
            WHERE workspace_id = $1 AND group_id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(group_id)
        .fetch_all(&mut **tx)
        .await?;
        candidates.extend(members.into_iter().map(|(id,)| id));
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
        let created_by: Option<(Uuid,)> = sqlx::query_as(
            "SELECT created_by FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some((created_by,)) = created_by {
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
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
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
    workspace_id: Uuid,
) -> Result<Vec<NotificationInsert>, sqlx::Error> {
    let Some(invitation_id) = event.target_id else {
        return Ok(Vec::new());
    };
    let invited_by: Option<(Uuid,)> =
        sqlx::query_as("SELECT invited_by FROM fvoci.invitations WHERE id = $1")
            .bind(invitation_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((invited_by,)) = invited_by else {
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
    let Some(workspace_id) = event.workspace_id else {
        return Ok(Vec::new());
    };
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
    let Some(comment) = load_comment(tx, workspace_id, comment_id).await? else {
        return Ok(Vec::new());
    };
    let recipients = comment_created_audience(tx, event, workspace_id, &comment).await?;
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
    let (subject, text_fn): (&str, fn(&str) -> String) = match event.verb.as_str() {
        "identity.linked" => (
            crate::mail::templates::IDENTITY_LINKED_SUBJECT,
            crate::mail::templates::identity_linked_text,
        ),
        "identity.unlinked" => (
            crate::mail::templates::IDENTITY_UNLINKED_SUBJECT,
            crate::mail::templates::identity_unlinked_text,
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
    Ok(Some(OutboundMail {
        to: email,
        subject: subject.to_string(),
        text: text_fn(provider),
    }))
}
