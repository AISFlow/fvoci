//! Atomic database outbox consumer that fans events into per-user in-app notifications.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::notifications::{
    list_task_assignees, load_comment, mail_messages, mail_user, notification_document_creator,
    notification_document_project, notification_group_members, notification_inviter,
    notification_project_name, resolved_store_prefs, status_name, task_snapshot, user_is_present,
    CommentSnap, NotificationInsert,
};
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
    let project = notification_document_project(tx, workspace, document).await?;
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
    comment_audience_with_current_target(tx, workspace_id, comment, members).await
}

async fn comment_audience_with_current_target(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    comment: &CommentSnap,
    members: Vec<Uuid>,
) -> Result<Vec<Uuid>, sqlx::Error> {
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
    let recipients =
        comment_audience_with_current_target(tx, workspace_id, &comment, recipients).await?;
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

pub(crate) async fn notify_for_event_backend(
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
    list_immediate_comment_mails_backend(&mut OperationTx::Postgres(tx), &event.clone().into())
        .await
}

pub(crate) async fn list_immediate_comment_mails_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
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
    if !tx.workspace_is_live(workspace_id).await? {
        return Ok(Vec::new());
    }
    let recipients = comment_created_audience(tx, event, workspace_id, &comment).await?;
    let actor_name = if let Some(actor_id) = event.actor_user_id {
        mail_user(tx, actor_id)
            .await?
            .map(|(_, given, family)| format_person_name_ko(&given, family.as_deref()))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let subject = crate::mail::templates::COMMENT_SUBJECT;
    let text = crate::mail::templates::comment_text(&actor_name, &comment.body);
    let mut mails = Vec::new();
    for user_id in recipients {
        let prefs = resolved_store_prefs(tx.notification_prefs(workspace_id, user_id).await?);
        if !prefs.mail_immediate {
            continue;
        }
        let Some((email, _, _)) = mail_user(tx, user_id).await? else {
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
    identity_mail_for_event_backend(&mut OperationTx::Postgres(tx), &event.clone().into()).await
}

pub(crate) async fn identity_mail_for_event_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
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
    let Some((email, _, _)) = mail_user(tx, actor_id).await? else {
        return Ok(None);
    };
    // Read at delivery time (a redelivery uses the then-current copy); the
    // caller commits this transaction before SMTP.
    let messages = mail_messages(tx).await?;
    Ok(Some(OutboundMail {
        to: email,
        subject: messages.subject(subject),
        text: messages.render(text, &[("provider", provider)]),
    }))
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
        f.grant_wiki().await;
        let resolved = f.append_comment_event("comment.resolved").await;
        process_notification_event_backend(&f.backend, owner, &resolved)
            .await
            .unwrap();
        assert_eq!(
            effect_counts(&f, resolved.id).await,
            (1, 1),
            "an authorized resolved comment must still produce its normal inbox effect"
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

#[cfg(test)]
mod immediate_mail_backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;

    async fn comment_mails(f: &Fixture, event: &BackendOutboxEvent) -> Vec<OutboundMail> {
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let mails = list_immediate_comment_mails_backend(&mut tx.operation(), event)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mails
    }

    #[tokio::test]
    async fn comment_mail_uses_current_audience_preferences_and_live_target() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        let mut event = f.append_comment_event("comment.created").await;
        // The cross-tenant account and actor must not become recipients.
        event.payload["mentionedUserIds"] = json!([f.user, f.other_user, f.actor, f.user]);
        let mails = comment_mails(&f, &event).await;
        assert_eq!(mails.len(), 1);
        assert_eq!(mails[0].to, "reader@notification.invalid");
        assert_eq!(mails[0].subject, crate::mail::templates::COMMENT_SUBJECT);
        assert_eq!(
            mails[0].text,
            crate::mail::templates::comment_text("한글🙂", "comment 한글🙂")
        );
        sqlx::query("INSERT INTO notification_prefs(workspace_id,user_id,in_app,mail_immediate,mail_digest) VALUES(?1,?2,1,0,0)").bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(comment_mails(&f, &event).await.is_empty());
        sqlx::query("UPDATE notification_prefs SET in_app=0,mail_immediate=1 WHERE workspace_id=?1 AND user_id=?2").bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            comment_mails(&f, &event).await.len(),
            1,
            "mail channel is independent of in-app preference"
        );
        sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            comment_mails(&f, &event).await.is_empty(),
            "grant loss must be applied to a retry"
        );
        f.grant_wiki().await;
        assert_eq!(comment_mails(&f, &event).await.len(), 1);
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            comment_mails(&f, &event).await.is_empty(),
            "membership loss must filter a still-granted target"
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn comment_mail_rejects_deleted_workspace_target_and_wrong_tenant() {
        let f = Fixture::new().await;
        f.grant_wiki().await;
        let mut event = f.append_comment_event("comment.created").await;
        assert_eq!(comment_mails(&f, &event).await.len(), 1);
        event.workspace_id = Some(f.other_workspace);
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        tx.operation().set_tenant(f.other_workspace).await.unwrap();
        assert!(
            list_immediate_comment_mails_backend(&mut tx.operation(), &event)
                .await
                .unwrap()
                .is_empty()
        );
        tx.commit().await.unwrap();
        event.workspace_id = Some(f.workspace);
        sqlx::query("UPDATE documents SET deleted_at=?1 WHERE id=?2")
            .bind(chrono::Utc::now().timestamp_micros())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(comment_mails(&f, &event).await.is_empty());
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(comment_mails(&f, &event).await.len(), 1);
        sqlx::query("UPDATE workspaces SET deleted_at=?1 WHERE id=?2")
            .bind(chrono::Utc::now().timestamp_micros())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(comment_mails(&f, &event).await.is_empty());
        f.finish().await;
    }

    async fn identity_mail(f: &Fixture, event: &BackendOutboxEvent) -> Option<OutboundMail> {
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        let mail = identity_mail_for_event_backend(&mut tx.operation(), event)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        mail
    }

    #[tokio::test]
    async fn identity_mail_reads_current_account_and_overrides_for_both_verbs() {
        use crate::settings::messages::Message;
        let f = Fixture::new().await;
        let mut event = f.append_comment_event("comment.created").await;
        event.workspace_id = None;
        event.verb = "identity.linked".into();
        event.payload = json!({"provider":"fixture-provider {{literal}}","email":"untrusted@notification.invalid"});
        let mail = identity_mail(&f, &event).await.unwrap();
        assert_eq!(mail.to, "actor@notification.invalid");
        assert_eq!(mail.subject, Message::IdentityLinkedSubject.default_text());
        assert!(mail.text.contains("fixture-provider {{literal}}"));
        sqlx::query(
            "UPDATE users SET email='current@notification.invalid',family_name='김' WHERE id=?1",
        )
        .bind(f.actor.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let overrides = json!({"overrides":{"mail.identity.unlinked.subject":"changed\nsubject","mail.identity.unlinked.text":"provider={{provider}}"}});
        sqlx::query("INSERT INTO instance_settings(key,value) VALUES('i18n',?1)")
            .bind(overrides.to_string())
            .execute(&f.pool)
            .await
            .unwrap();
        event.verb = "identity.unlinked".into();
        let mail = identity_mail(&f, &event).await.unwrap();
        assert_eq!(mail.to, "current@notification.invalid");
        assert_eq!(mail.subject, "changed subject");
        assert_eq!(mail.text, "provider=fixture-provider {{literal}}");
        event.payload = json!({"provider":null});
        assert!(identity_mail(&f, &event).await.is_none());
        event.payload = json!({"provider":"fixture"});
        event.actor_user_id = None;
        assert!(identity_mail(&f, &event).await.is_none());
        event.actor_user_id = Some(f.actor);
        sqlx::query("UPDATE users SET deleted_at=?1 WHERE id=?2")
            .bind(chrono::Utc::now().timestamp_micros())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(identity_mail(&f, &event).await.is_none());
        f.finish().await;
    }
}
