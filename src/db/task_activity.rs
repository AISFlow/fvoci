use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::comments::{comment_output, CommentRow};
use crate::db::context::set_tenant;
use crate::db::projects::{lock_project, project_permission};
use crate::db::tasks::{list_task_assignee_ids, list_task_label_ids, TaskRowRecord};
use crate::db::workspace::WorkspaceRole;
use crate::projects::{effective_permission, ProjectMemberRole, ProjectPermission};
use crate::tasks::activity::{
    activity_scope, decode_activity_cursor, diff_activity, encode_activity_cursor,
    normalize_estimate, ActivityFilter, ActivityListQuery, ActivitySnapshot,
};

#[derive(Debug)]
pub enum TaskActivityDbError {
    NotFound,
    InvalidInput,
    InvalidCursor,
}

#[derive(Debug, Clone)]
pub struct TaskActivityRow {
    pub id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub channel: String,
    pub kind: String,
    pub changes: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TaskActivityListPage {
    pub items: Vec<TaskActivityOutputItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TaskActivityOutputItem {
    Change(TaskActivityChangeOutput),
    Comment(TaskActivityCommentOutput),
}

#[derive(Debug, Clone)]
pub struct TaskActivityChangeOutput {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub actor: Option<ActivityActorOutput>,
    pub channel: String,
    pub kind: String,
    pub changes: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct TaskActivityCommentOutput {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub actor: Option<ActivityActorOutput>,
    pub comment: Value,
    pub parent: Option<TaskActivityCommentParentOutput>,
}

#[derive(Debug, Clone)]
pub struct TaskActivityCommentParentOutput {
    pub id: Uuid,
    pub body: String,
    pub actor: Option<ActivityActorOutput>,
}

#[derive(Debug, Clone)]
pub struct ActivityActorOutput {
    pub id: Uuid,
    pub name: String,
}

type ParentVisibilityRow = (Uuid, Option<DateTime<Utc>>, String, Option<String>, String);

struct FeedPosition {
    id: Uuid,
    item_type: String,
}

pub async fn record_task_activity(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    channel: &str,
    before: Option<&ActivitySnapshot>,
    after: &ActivitySnapshot,
) -> Result<(), sqlx::Error> {
    let kind = if before.is_none() {
        "created"
    } else {
        "changed"
    };
    let changes = match diff_activity(before, after) {
        Some(changes) => Value::Array(changes),
        None if before.is_some() => return Ok(()),
        None => Value::Array(vec![]),
    };
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_activity (
            id, workspace_id, task_id, actor_user_id, channel, kind, changes
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_id)
    .bind(actor_user_id)
    .bind(channel)
    .bind(kind)
    .bind(changes)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(private_interfaces)]
pub async fn task_activity_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task: &TaskRowRecord,
    recurrence: Option<&Value>,
    fields: &[&str],
) -> Result<ActivitySnapshot, sqlx::Error> {
    let requested: HashSet<&str> = fields
        .iter()
        .map(|field| {
            if *field == "archivedAt" {
                "archived"
            } else {
                *field
            }
        })
        .collect();
    let mut snapshot = ActivitySnapshot::new();
    for field in [
        "title",
        "type",
        "priority",
        "startDate",
        "dueDate",
        "estimate",
    ] {
        if !requested.contains(field) {
            continue;
        }
        match field {
            "title" => snapshot.insert(field.to_string(), json!(task.title)),
            "type" => snapshot.insert(field.to_string(), json!(task.task_type)),
            "priority" => snapshot.insert(field.to_string(), json!(task.priority)),
            "startDate" => snapshot.insert(
                field.to_string(),
                task.start_date
                    .map(|d| json!(d.to_string()))
                    .unwrap_or(Value::Null),
            ),
            "dueDate" => snapshot.insert(
                field.to_string(),
                task.due_date
                    .map(|d| json!(d.to_string()))
                    .unwrap_or(Value::Null),
            ),
            "estimate" => snapshot.insert(
                field.to_string(),
                task.estimate
                    .as_ref()
                    .map(|value| json!(normalize_estimate(value)))
                    .unwrap_or(Value::Null),
            ),
            _ => None,
        };
    }
    if requested.contains("dueAt") {
        snapshot.insert(
            "dueAt".to_string(),
            task.due_at
                .map(|value| json!(value.to_rfc3339()))
                .unwrap_or(Value::Null),
        );
    }
    if requested.contains("archived") {
        snapshot.insert("archived".to_string(), json!(task.archived_at.is_some()));
    }
    if requested.contains("recurrence") {
        let kind = recurrence
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str);
        snapshot.insert(
            "recurrence".to_string(),
            kind.map(|value| json!(value)).unwrap_or(Value::Null),
        );
    }
    if requested.contains("statusId") {
        let label: Option<String> = sqlx::query_scalar(
            r#"
            SELECT name
            FROM fvoci.statuses
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(task.status_id)
        .fetch_optional(&mut **tx)
        .await?;
        snapshot.insert(
            "statusId".to_string(),
            json!({ "id": task.status_id.to_string(), "label": label }),
        );
    }
    if requested.contains("milestoneId") {
        let milestone = if task.milestone_id.is_some() {
            sqlx::query_as::<_, (Uuid, String)>(
                r#"
                SELECT id, name
                FROM fvoci.milestones
                WHERE workspace_id = $1 AND project_id = $2 AND id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(task.project_id)
            .bind(task.milestone_id)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            None
        };
        snapshot.insert(
            "milestoneId".to_string(),
            match milestone {
                Some((id, name)) => json!({ "id": id.to_string(), "label": name }),
                None => Value::Null,
            },
        );
    }
    if requested.contains("parentId") {
        snapshot.insert(
            "parentId".to_string(),
            task.parent_id
                .map(|id| json!({ "id": id.to_string(), "label": Value::Null }))
                .unwrap_or(Value::Null),
        );
    }
    if requested.contains("assigneeIds") {
        let ids = list_task_assignee_ids(tx, workspace_id, task.id).await?;
        snapshot.insert(
            "assigneeIds".to_string(),
            json!(ids
                .iter()
                .map(|id| json!({ "id": id.to_string(), "label": Value::Null }))
                .collect::<Vec<_>>()),
        );
    }
    if requested.contains("labelIds") {
        let ids = list_task_label_ids(tx, workspace_id, task.id).await?;
        let labels = if ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as::<_, (Uuid, String)>(
                r#"
                SELECT id, name
                FROM fvoci.labels
                WHERE workspace_id = $1 AND project_id = $2 AND id = ANY($3)
                "#,
            )
            .bind(workspace_id)
            .bind(task.project_id)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?
        };
        let names: HashMap<Uuid, String> = labels.into_iter().collect();
        snapshot.insert(
            "labelIds".to_string(),
            json!(ids
                .iter()
                .map(|id| {
                    json!({
                        "id": id.to_string(),
                        "label": names.get(id).cloned()
                    })
                })
                .collect::<Vec<_>>()),
        );
    }
    Ok(snapshot)
}

pub async fn list_task_activity(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    query: ActivityListQuery,
) -> Result<Result<TaskActivityListPage, TaskActivityDbError>, sqlx::Error> {
    if !(1..=100).contains(&query.limit) {
        return Ok(Err(TaskActivityDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !crate::db::context::session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }
    if !crate::db::documents::workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }
    let task_row: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id, deleted_at)) = task_row else {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    };
    if deleted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }

    let scope = activity_scope(workspace_id, task_id, query.filter);
    let after_payload = match &query.cursor {
        Some(cursor) => match decode_activity_cursor(cursor, &scope) {
            Ok(value) => Some(value),
            Err(_) => {
                tx.rollback().await?;
                return Ok(Err(TaskActivityDbError::InvalidCursor));
            }
        },
        None => None,
    };

    let include_changes = query.filter != ActivityFilter::Comments;
    let include_comments = query.filter != ActivityFilter::Changes;
    let limit = query.limit + 1;
    let positions = if let Some(after) = after_payload {
        sqlx::query(
            r#"
            SELECT id, type, created_at,
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS at
            FROM (
                SELECT id, 'change'::text AS type, created_at
                FROM fvoci.task_activity
                WHERE workspace_id = $1 AND task_id = $2 AND $4
                UNION ALL
                SELECT id, 'comment'::text AS type, created_at
                FROM fvoci.comments
                WHERE workspace_id = $1 AND task_id = $2 AND $5
            ) AS feed
            WHERE (created_at, id, type) < ($6::timestamptz, $7::uuid, $8::text)
            ORDER BY created_at DESC, id DESC, type DESC
            LIMIT $3
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(limit)
        .bind(include_changes)
        .bind(include_comments)
        .bind(
            DateTime::parse_from_rfc3339(&after.at)
                .map_err(|_| sqlx::Error::Protocol("bad cursor at".into()))?
                .with_timezone(&Utc),
        )
        .bind(after.id)
        .bind(after.item_type)
        .fetch_all(&mut *tx)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT id, type, created_at,
                to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS at
            FROM (
                SELECT id, 'change'::text AS type, created_at
                FROM fvoci.task_activity
                WHERE workspace_id = $1 AND task_id = $2 AND $4
                UNION ALL
                SELECT id, 'comment'::text AS type, created_at
                FROM fvoci.comments
                WHERE workspace_id = $1 AND task_id = $2 AND $5
            ) AS feed
            ORDER BY created_at DESC, id DESC, type DESC
            LIMIT $3
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(limit)
        .bind(include_changes)
        .bind(include_comments)
        .fetch_all(&mut *tx)
        .await?
    };

    let mut feed_positions = Vec::new();
    for row in positions {
        feed_positions.push(FeedPosition {
            id: row.get("id"),
            item_type: row.get("type"),
        });
    }
    let fetched_has_more = feed_positions.len() > query.limit as usize;
    if fetched_has_more {
        feed_positions.truncate(query.limit as usize);
    }

    let change_ids = feed_positions
        .iter()
        .filter(|p| p.item_type == "change")
        .map(|p| p.id)
        .collect::<Vec<_>>();
    let comment_ids = feed_positions
        .iter()
        .filter(|p| p.item_type == "comment")
        .map(|p| p.id)
        .collect::<Vec<_>>();

    let mut changes_by_id = HashMap::new();
    if !change_ids.is_empty() {
        let rows = sqlx::query(
            r#"
            SELECT id, actor_user_id, channel, kind, changes, created_at
            FROM fvoci.task_activity
            WHERE workspace_id = $1 AND task_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(&change_ids)
        .fetch_all(&mut *tx)
        .await?;
        for row in rows {
            changes_by_id.insert(
                row.get::<Uuid, _>("id"),
                TaskActivityRow {
                    id: row.get("id"),
                    actor_user_id: row.get("actor_user_id"),
                    channel: row.get("channel"),
                    kind: row.get("kind"),
                    changes: row.get("changes"),
                    created_at: row.get("created_at"),
                },
            );
        }
    }

    let mut comments_by_id = HashMap::new();
    if !comment_ids.is_empty() {
        let rows = sqlx::query(
            r#"
            SELECT id, workspace_id, document_id, task_id, parent_id, created_by, body,
                   resolved_at, reactions, created_at, updated_at
            FROM fvoci.comments
            WHERE workspace_id = $1 AND task_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(&comment_ids)
        .fetch_all(&mut *tx)
        .await?;
        for row in rows {
            let comment = CommentRow {
                id: row.get("id"),
                workspace_id: row.get("workspace_id"),
                document_id: row.get("document_id"),
                task_id: row.get("task_id"),
                parent_id: row.get("parent_id"),
                created_by: row.get("created_by"),
                body: row.get("body"),
                resolved_at: row.get("resolved_at"),
                reactions: row.get("reactions"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            };
            comments_by_id.insert(comment.id, comment);
        }
    }
    let parent_ids = comments_by_id
        .values()
        .filter_map(|comment| comment.parent_id)
        .collect::<HashSet<_>>();
    let mut parents_by_id = HashMap::new();
    if !parent_ids.is_empty() {
        let ids = parent_ids.into_iter().collect::<Vec<_>>();
        let rows = sqlx::query(
            r#"
            SELECT id, workspace_id, document_id, task_id, parent_id, created_by, body,
                   resolved_at, reactions, created_at, updated_at
            FROM fvoci.comments
            WHERE workspace_id = $1 AND task_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
        for row in rows {
            let comment = CommentRow {
                id: row.get("id"),
                workspace_id: row.get("workspace_id"),
                document_id: row.get("document_id"),
                task_id: row.get("task_id"),
                parent_id: row.get("parent_id"),
                created_by: row.get("created_by"),
                body: row.get("body"),
                resolved_at: row.get("resolved_at"),
                reactions: row.get("reactions"),
                created_at: row.get("created_at"),
                updated_at: row.get("updated_at"),
            };
            parents_by_id.insert(comment.id, comment);
        }
    }

    let mut people_ids = HashSet::new();
    for row in changes_by_id.values() {
        if let Some(actor) = row.actor_user_id {
            people_ids.insert(actor);
        }
        if let Some(changes) = row.changes.as_array() {
            for change in changes {
                if change.get("field").and_then(Value::as_str) == Some("assigneeIds") {
                    for key in ["from", "to"] {
                        if let Some(Value::Array(items)) = change.get(key) {
                            for item in items.iter().take(50) {
                                if let Some(id) = item.get("id").and_then(Value::as_str) {
                                    if let Ok(uuid) = Uuid::parse_str(id) {
                                        people_ids.insert(uuid);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    for comment in comments_by_id.values() {
        people_ids.insert(comment.created_by);
        if let Some(parent_id) = comment.parent_id {
            if let Some(parent) = parents_by_id.get(&parent_id) {
                people_ids.insert(parent.created_by);
            }
        }
    }
    let people_ids = people_ids.into_iter().collect::<Vec<_>>();
    let people = load_activity_people(&mut tx, workspace_id, &people_ids).await?;
    let mut parent_titles: HashMap<Uuid, Option<String>> = HashMap::new();

    let mut items = Vec::new();
    let mut bytes = 2048usize;
    let mut truncated_by_budget = false;
    for position in &feed_positions {
        let item = if position.item_type == "change" {
            let Some(activity) = changes_by_id.get(&position.id) else {
                tx.rollback().await?;
                return Err(sqlx::Error::RowNotFound);
            };
            let changes = enrich_changes(
                &mut tx,
                workspace_id,
                actor_user_id,
                activity.changes.clone(),
                &people,
                &mut parent_titles,
            )
            .await?;
            TaskActivityOutputItem::Change(TaskActivityChangeOutput {
                id: activity.id,
                created_at: activity.created_at,
                actor: actor_output(activity.actor_user_id, &people),
                channel: activity.channel.clone(),
                kind: activity.kind.clone(),
                changes,
            })
        } else {
            let Some(comment) = comments_by_id.get(&position.id) else {
                tx.rollback().await?;
                return Err(sqlx::Error::RowNotFound);
            };
            let (reactions, other_reaction_count) = comment_output(comment, actor_user_id);
            let comment_value = json!({
                "id": comment.id,
                "workspaceId": comment.workspace_id,
                "documentId": comment.document_id,
                "taskId": comment.task_id,
                "parentId": comment.parent_id,
                "createdBy": comment.created_by,
                "body": comment.body,
                "resolvedAt": comment.resolved_at,
                "reactions": reactions,
                "otherReactionCount": other_reaction_count,
                "createdAt": comment.created_at,
                "updatedAt": comment.updated_at,
            });
            let parent = comment.parent_id.and_then(|parent_id| {
                parents_by_id
                    .get(&parent_id)
                    .map(|parent| TaskActivityCommentParentOutput {
                        id: parent.id,
                        body: parent.body.chars().take(200).collect(),
                        actor: actor_output(Some(parent.created_by), &people),
                    })
            });
            TaskActivityOutputItem::Comment(TaskActivityCommentOutput {
                id: comment.id,
                created_at: comment.created_at,
                actor: actor_output(Some(comment.created_by), &people),
                comment: comment_value,
                parent,
            })
        };
        let encoded = activity_item_bytes(&item);
        let item_bytes = encoded.len() + 1;
        if items.len() > 1 && bytes + item_bytes > 1_048_576 {
            truncated_by_budget = true;
            break;
        }
        bytes += item_bytes;
        items.push(item);
    }

    let next_cursor = if fetched_has_more || truncated_by_budget {
        items.last().map(|item| {
            let (id, created_at, item_type) = match item {
                TaskActivityOutputItem::Change(change) => (change.id, change.created_at, "change"),
                TaskActivityOutputItem::Comment(comment) => {
                    (comment.id, comment.created_at, "comment")
                }
            };
            encode_activity_cursor(id, created_at, item_type, &scope)
        })
    } else {
        None
    };

    tx.commit().await?;
    Ok(Ok(TaskActivityListPage { items, next_cursor }))
}

fn activity_item_bytes(item: &TaskActivityOutputItem) -> Vec<u8> {
    match item {
        TaskActivityOutputItem::Change(change) => serde_json::to_vec(&json!({
            "type": "change",
            "id": change.id,
            "createdAt": change.created_at,
            "changes": change.changes,
        }))
        .expect("activity change bytes"),
        TaskActivityOutputItem::Comment(comment) => serde_json::to_vec(&json!({
            "type": "comment",
            "id": comment.id,
            "createdAt": comment.created_at,
            "comment": comment.comment,
        }))
        .expect("activity comment bytes"),
    }
}

async fn load_activity_people(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, ActivityActorOutput>, sqlx::Error> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT u.id, u.given_name, u.family_name
        FROM fvoci.users u
        INNER JOIN fvoci.memberships m ON m.user_id = u.id
        WHERE m.workspace_id = $1
          AND u.id = ANY($2)
          AND u.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(user_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let id = row.get::<Uuid, _>("id");
        let given = row.get::<String, _>("given_name");
        let family = row.get::<Option<String>, _>("family_name");
        let name = match family {
            Some(family) if !family.is_empty() => format!("{given} {family}"),
            _ => given,
        };
        out.insert(id, ActivityActorOutput { id, name });
    }
    Ok(out)
}

fn actor_output(
    user_id: Option<Uuid>,
    people: &HashMap<Uuid, ActivityActorOutput>,
) -> Option<ActivityActorOutput> {
    user_id.and_then(|id| people.get(&id).cloned())
}

async fn enrich_changes(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    changes: Value,
    people: &HashMap<Uuid, ActivityActorOutput>,
    parent_titles: &mut HashMap<Uuid, Option<String>>,
) -> Result<Vec<Value>, sqlx::Error> {
    let Some(items) = changes.as_array() else {
        return Ok(vec![]);
    };
    let mut out = Vec::with_capacity(items.len());
    for change in items {
        let field = change.get("field").and_then(Value::as_str).unwrap_or("");
        let from = visible_value(
            tx,
            workspace_id,
            actor_user_id,
            field,
            change.get("from").cloned().unwrap_or(Value::Null),
            people,
            parent_titles,
        )
        .await?;
        let to = visible_value(
            tx,
            workspace_id,
            actor_user_id,
            field,
            change.get("to").cloned().unwrap_or(Value::Null),
            people,
            parent_titles,
        )
        .await?;
        let next = change.clone();
        if let Some(obj) = next.as_object() {
            let mut map = obj.clone();
            map.insert("from".to_string(), from);
            map.insert("to".to_string(), to);
            out.push(Value::Object(map));
        }
    }
    Ok(out)
}

async fn visible_value(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    field: &str,
    value: Value,
    people: &HashMap<Uuid, ActivityActorOutput>,
    parent_titles: &mut HashMap<Uuid, Option<String>>,
) -> Result<Value, sqlx::Error> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    if let Some(items) = value.as_array() {
        let total = items.len();
        let mapped = items
            .iter()
            .take(50)
            .map(|item| {
                if field == "assigneeIds" {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .and_then(|raw| Uuid::parse_str(raw).ok());
                    json!({
                        "id": item.get("id").cloned().unwrap_or(Value::Null),
                        "label": id
                            .and_then(|id| people.get(&id).map(|person| json!(person.name)))
                            .unwrap_or(Value::Null),
                    })
                } else {
                    item.clone()
                }
            })
            .collect::<Vec<_>>();
        return Ok(json!({ "items": mapped, "totalCount": total }));
    }
    if field == "parentId" {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .and_then(|raw| Uuid::parse_str(raw).ok());
        if let Some(parent_id) = id {
            if let std::collections::hash_map::Entry::Vacant(entry) = parent_titles.entry(parent_id)
            {
                let row: Option<ParentVisibilityRow> = sqlx::query_as(
                    r#"
                        SELECT t.project_id, t.deleted_at, p.visibility, pm.role, m.role
                        FROM fvoci.tasks t
                        INNER JOIN fvoci.projects p
                          ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                        INNER JOIN fvoci.memberships m
                          ON m.workspace_id = p.workspace_id AND m.user_id = $3
                        LEFT JOIN fvoci.project_members pm
                          ON pm.workspace_id = p.workspace_id
                         AND pm.project_id = p.id
                         AND pm.user_id = $3
                        WHERE t.workspace_id = $1 AND t.id = $2
                        "#,
                )
                .bind(workspace_id)
                .bind(parent_id)
                .bind(actor_user_id)
                .fetch_optional(&mut **tx)
                .await?;
                let title = if let Some((
                    _project_id,
                    deleted_at,
                    visibility,
                    member_role,
                    workspace_role,
                )) = row
                {
                    if deleted_at.is_some() {
                        None
                    } else {
                        let permission = effective_permission(
                            WorkspaceRole::parse(&workspace_role).unwrap_or(WorkspaceRole::Guest),
                            &visibility,
                            member_role.as_deref().and_then(ProjectMemberRole::parse),
                        );
                        if permission.at_least(ProjectPermission::View) {
                            sqlx::query_scalar(
                                "SELECT title FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
                            )
                            .bind(workspace_id)
                            .bind(parent_id)
                            .fetch_optional(&mut **tx)
                            .await?
                        } else {
                            None
                        }
                    }
                } else {
                    None
                };
                entry.insert(title);
            }
            return Ok(json!({
                "id": parent_id.to_string(),
                "label": parent_titles.get(&parent_id).cloned().unwrap_or(None),
            }));
        }
    }
    Ok(value)
}
