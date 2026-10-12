use std::collections::{HashMap, HashSet};

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::comments::{row_to_comment, CommentRow};
use crate::db::context::{begin_read, set_tenant};
use crate::db::projects::{load_live_project, project_permission, project_permission_by_id};
use crate::db::tasks::{list_task_assignee_ids, list_task_label_ids, TaskRowRecord};
use crate::projects::ProjectPermission;
use crate::tasks::activity::{
    activity_scope, decode_activity_cursor, diff_activity, normalize_estimate, ActivityFilter,
    ActivityListQuery, ActivitySnapshot,
};

#[derive(Debug)]
pub enum TaskActivityDbError {
    NotFound,
    InvalidInput,
    InvalidCursor,
}

/// One page of the task feed before response shaping. `has_more` means a row
/// exists past `items`; the route applies the response byte budget and encodes
/// the cursor with `scope`.
#[derive(Debug, Clone)]
pub struct TaskActivityListPage {
    pub items: Vec<TaskActivityOutputItem>,
    pub has_more: bool,
    pub scope: String,
}

#[derive(Debug, Clone)]
pub enum TaskActivityOutputItem {
    Change(TaskActivityChangeOutput),
    Comment(TaskActivityCommentOutput),
}

impl TaskActivityOutputItem {
    pub fn position(&self) -> (Uuid, DateTime<Utc>, &'static str) {
        match self {
            Self::Change(change) => (change.id, change.created_at, "change"),
            Self::Comment(comment) => (comment.comment.id, comment.comment.created_at, "comment"),
        }
    }
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
    pub actor: Option<ActivityActorOutput>,
    pub comment: CommentRow,
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
                .map(|value| json!(value.to_rfc3339_opts(SecondsFormat::Millis, true)))
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
    let scope = activity_scope(workspace_id, task_id, query.filter);
    let after = match query.cursor.as_deref() {
        Some(raw) => match decode_activity_cursor(raw, &scope) {
            Ok(cursor) => Some(cursor),
            Err(_) => return Ok(Err(TaskActivityDbError::InvalidCursor)),
        },
        None => None,
    };

    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !crate::db::context::session_is_live(&mut tx, actor_user_id, session_id).await?
        || !crate::db::workspace::workspace_is_live(&mut tx, workspace_id).await?
    {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }
    let project_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(project_id) = project_id else {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    };
    let Some(project) = load_live_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    };
    let permission = project_permission(&mut tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Err(TaskActivityDbError::NotFound));
    }

    let positions: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, type
        FROM (
            SELECT id, 'change'::text AS type, created_at
            FROM fvoci.task_activity
            WHERE workspace_id = $1 AND task_id = $2 AND $4
            UNION ALL
            SELECT id, 'comment'::text AS type, created_at
            FROM fvoci.comments
            WHERE workspace_id = $1 AND task_id = $2 AND $5
        ) AS feed
        WHERE $6::timestamptz IS NULL
           OR (created_at, id, type) < ($6::timestamptz, $7::uuid, $8::text)
        ORDER BY created_at DESC, id DESC, type DESC
        LIMIT $3
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(i64::from(query.limit) + 1)
    .bind(query.filter != ActivityFilter::Comments)
    .bind(query.filter != ActivityFilter::Changes)
    .bind(after.as_ref().map(|cursor| cursor.at))
    .bind(after.as_ref().map(|cursor| cursor.id))
    .bind(after.as_ref().map(|cursor| cursor.item_type.as_str()))
    .fetch_all(&mut *tx)
    .await?;
    let has_more = positions.len() > query.limit as usize;
    let positions = &positions[..positions.len().min(query.limit as usize)];

    let ids_of = |kind: &str| {
        positions
            .iter()
            .filter(|(_, item_type)| item_type == kind)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
    };
    let changes_by_id = load_changes(&mut tx, workspace_id, task_id, &ids_of("change")).await?;
    let comments_by_id = load_comments(&mut tx, workspace_id, task_id, &ids_of("comment")).await?;
    let parent_ids = comments_by_id
        .values()
        .filter_map(|comment| comment.parent_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let parents_by_id = load_comments(&mut tx, workspace_id, task_id, &parent_ids).await?;

    let mut people_ids = HashSet::new();
    for row in changes_by_id.values() {
        people_ids.extend(row.actor_user_id);
        for change in row.changes.as_array().into_iter().flatten() {
            if change.get("field").and_then(Value::as_str) != Some("assigneeIds") {
                continue;
            }
            for key in ["from", "to"] {
                for item in change
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .take(50)
                {
                    people_ids.extend(
                        item.get("id")
                            .and_then(Value::as_str)
                            .and_then(|raw| Uuid::parse_str(raw).ok()),
                    );
                }
            }
        }
    }
    for comment in comments_by_id.values().chain(parents_by_id.values()) {
        people_ids.insert(comment.created_by);
    }
    let people_ids = people_ids.into_iter().collect::<Vec<_>>();
    let people = load_activity_people(&mut tx, workspace_id, &people_ids).await?;
    let mut parent_titles = ParentTitles {
        project_id,
        project_permission: permission,
        titles: HashMap::new(),
    };

    let mut items = Vec::with_capacity(positions.len());
    for (id, item_type) in positions {
        let item = if item_type == "change" {
            let Some(activity) = changes_by_id.get(id) else {
                tx.rollback().await?;
                return Err(sqlx::Error::RowNotFound);
            };
            let changes = enrich_changes(
                &mut tx,
                workspace_id,
                actor_user_id,
                &activity.changes,
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
            let Some(comment) = comments_by_id.get(id) else {
                tx.rollback().await?;
                return Err(sqlx::Error::RowNotFound);
            };
            let parent = comment
                .parent_id
                .and_then(|parent_id| parents_by_id.get(&parent_id))
                .map(|parent| TaskActivityCommentParentOutput {
                    id: parent.id,
                    body: parent.body.chars().take(200).collect(),
                    actor: actor_output(Some(parent.created_by), &people),
                });
            TaskActivityOutputItem::Comment(TaskActivityCommentOutput {
                actor: actor_output(Some(comment.created_by), &people),
                comment: comment.clone(),
                parent,
            })
        };
        items.push(item);
    }

    tx.commit().await?;
    Ok(Ok(TaskActivityListPage {
        items,
        has_more,
        scope,
    }))
}

pub async fn list_task_activity_backend(
    backend: &crate::db::backend::Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    task: Uuid,
    query: ActivityListQuery,
) -> Result<Result<TaskActivityListPage, TaskActivityDbError>, sqlx::Error> {
    use crate::db::backend::{Backend, DbTx, OperationTx};
    use crate::db::codec::Cell;
    if let Backend::Postgres(pool) = backend {
        return list_task_activity(pool, workspace, actor, credential, task, query).await;
    }
    if !(1..=100).contains(&query.limit) {
        return Ok(Err(TaskActivityDbError::InvalidInput));
    }
    let scope = activity_scope(workspace, task, query.filter);
    let after = match query.cursor.as_deref() {
        Some(raw) => match decode_activity_cursor(raw, &scope) {
            Ok(cursor) => Some(cursor),
            Err(_) => return Ok(Err(TaskActivityDbError::InvalidCursor)),
        },
        None => None,
    };
    let mut tx = backend.begin_read().await?;
    let result=async {
        let DbTx::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        if crate::db::task_ops::family_time_capability(family, workspace, task, actor, credential)
            .await?
            .is_err()
        {
            return Ok(Err(TaskActivityDbError::NotFound));
        }
        let positions=family.query("SELECT id,kind FROM (SELECT id,'change' AS kind,created_at FROM task_activity WHERE workspace_id=?1 AND task_id=?2 AND ?4 UNION ALL SELECT id,'comment' AS kind,created_at FROM comments WHERE workspace_id=?1 AND task_id=?2 AND ?5) WHERE ?6 IS NULL OR (created_at,id,kind)<(?6,?7,?8) ORDER BY created_at DESC,id DESC,kind DESC LIMIT ?3",&[Cell::uuid(workspace),Cell::uuid(task),Cell::Integer(i64::from(query.limit)+1),Cell::Integer(i64::from(query.filter!=ActivityFilter::Comments)),Cell::Integer(i64::from(query.filter!=ActivityFilter::Changes)),after.as_ref().map(|c|Cell::instant(c.at)).transpose()?.unwrap_or(Cell::Null),Cell::optional_uuid(after.as_ref().map(|c|c.id)),Cell::optional_text(after.as_ref().map(|c|c.item_type.as_str()))]).await?;

        let has_more = positions.len() > query.limit as usize;
        let mut changes = HashMap::new();
        let mut comments = HashMap::new();
        let mut people_ids = HashSet::new();
        let mut ordered = Vec::new();
        for position in positions.iter().take(query.limit as usize) {
            let id = position.cell(0)?.id()?;
            let kind = position.cell(1)?.string()?;
            if kind == "change" {
                let rows=family.query("SELECT id,actor_user_id,channel,kind,changes,created_at FROM task_activity WHERE workspace_id=?1 AND task_id=?2 AND id=?3",&[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(id)]).await?;
                let r = rows.first().ok_or(sqlx::Error::RowNotFound)?;

                let activity = TaskActivityRow {
                    id,
                    actor_user_id: r.cell(1)?.optional(Cell::id)?,
                    channel: r.cell(2)?.string()?,
                    kind: r.cell(3)?.string()?,
                    changes: r.cell(4)?.value()?,
                    created_at: r.cell(5)?.datetime()?,
                };
                people_ids.extend(activity.actor_user_id);

                for change in activity.changes.as_array().into_iter().flatten() {
                    if change.get("field").and_then(Value::as_str) == Some("assigneeIds") {
                        for key in ["from", "to"] {
                            for item in change
                                .get(key)
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .take(50)
                            {
                                people_ids.extend(
                                    item.get("id")
                                        .and_then(Value::as_str)
                                        .and_then(|s| Uuid::parse_str(s).ok()),
                                );
                            }
                        }
                    }
                }

                changes.insert(id, activity);
            } else {
                let comment = family_activity_comment(family, workspace, task, id)
                    .await?
                    .ok_or(sqlx::Error::RowNotFound)?;
                people_ids.insert(comment.created_by);
                comments.insert(id, comment);
            }

            ordered.push((id, kind));
        }
        let mut parents = HashMap::new();
        for parent in comments
            .values()
            .filter_map(|c| c.parent_id)
            .collect::<HashSet<_>>()
        {
            if let Some(comment) = family_activity_comment(family, workspace, task, parent).await? {
                people_ids.insert(comment.created_by);
                parents.insert(parent, comment);
            }
        }

        let mut people = HashMap::new();
        for id in people_ids {
            let rows=family.query("SELECT u.given_name,u.family_name FROM users u JOIN memberships m ON m.user_id=u.id WHERE m.workspace_id=?1 AND u.id=?2 AND u.deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(id)]).await?;
            if let Some(row) = rows.first() {
                let given = row.cell(0)?.string()?;
                let surname = row.cell(1)?.optional(Cell::string)?;
                let name = match surname {
                    Some(s) if !s.is_empty() => format!("{given} {s}"),
                    _ => given,
                };
                people.insert(id, ActivityActorOutput { id, name });
            }
        }

        let mut items = Vec::new();
        for (id, kind) in ordered {
            if kind == "change" {
                let activity = changes.remove(&id).ok_or(sqlx::Error::RowNotFound)?;
                let mut enriched = Vec::new();
                for change in activity.changes.as_array().into_iter().flatten() {
                    let Some(obj) = change.as_object() else {
                        continue;
                    };
                    let field = obj.get("field").and_then(Value::as_str).unwrap_or("");
                    let mut map = obj.clone();
                    for key in ["from", "to"] {
                        let value = obj.get(key).cloned().unwrap_or(Value::Null);
                        let value = if let Some(values) = value.as_array() {
                            let mapped=values.iter().take(50).map(|item|if field!="assigneeIds"{item.clone()}
    else{json!({"id":item.get("id").cloned().unwrap_or(Value::Null),"label":item.get("id").and_then(Value::as_str).and_then(|s|Uuid::parse_str(s).ok()).and_then(|id|people.get(&id)).map(|p|p.name.clone())}
    )}
    ).collect::<Vec<_>>();
                            json!({"items":mapped,"totalCount":values.len()}
                            )
                        } else if field == "parentId" {
                            if let Some(parent) = value
                                .get("id")
                                .and_then(Value::as_str)
                                .and_then(|s| Uuid::parse_str(s).ok())
                            {
                                let rows=family.query("SELECT project_id,title FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(parent)]).await?;
                                let title = if let Some(row) = rows.first() {
                                    if OperationTx::SqliteFamily(&mut *family)
                                        .project_permission_by_id(workspace, actor, row.cell(0)?.id()?)
                                        .await?
                                        .is_some_and(|p| p.at_least(ProjectPermission::View))
                                    {
                                        Some(row.cell(1)?.string()?)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                json!({"id":parent.to_string(),"label":title}
                                )
                            } else {
                                value
                            }
                        } else {
                            value
                        };
                        map.insert(key.into(), value);
                    }
                    enriched.push(Value::Object(map));
                }

                items.push(TaskActivityOutputItem::Change(TaskActivityChangeOutput {
                    id,
                    created_at: activity.created_at,
                    actor: actor_output(activity.actor_user_id, &people),
                    channel: activity.channel,
                    kind: activity.kind,
                    changes: enriched,
                }));
            } else {
                let comment = comments.remove(&id).ok_or(sqlx::Error::RowNotFound)?;
                let parent = comment.parent_id.and_then(|id| parents.get(&id)).map(|p| {
                    TaskActivityCommentParentOutput {
                        id: p.id,
                        body: p.body.chars().take(200).collect(),
                        actor: actor_output(Some(p.created_by), &people),
                    }
                });
                items.push(TaskActivityOutputItem::Comment(TaskActivityCommentOutput {
                    actor: actor_output(Some(comment.created_by), &people),
                    comment,
                    parent,
                }));
            }
        }

        Ok(Ok(TaskActivityListPage {
            items,
            has_more,
            scope,
        }))
    }.await;
    // Reads release the original snapshot on all paths, retaining cleanup failures.
    if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Err(e) => Some(Box::new(e)),
            Ok(Err(e)) => Some(Box::new(ActivityReadRefusal(e))),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}
#[derive(Debug, thiserror::Error)]
#[error("task activity refused: {0:?}")]
struct ActivityReadRefusal(TaskActivityDbError);
async fn family_activity_comment(
    tx: &mut crate::db::backend::FamilyTx,
    workspace: Uuid,
    task: Uuid,
    id: Uuid,
) -> Result<Option<CommentRow>, sqlx::Error> {
    use crate::db::codec::Cell;
    tx.query("SELECT id,workspace_id,document_id,task_id,parent_id,created_by,body,resolved_at,reactions,created_at,updated_at FROM comments WHERE workspace_id=?1 AND task_id=?2 AND id=?3",&[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(id)]).await?.first().map(|r|Ok(CommentRow{id:r.cell(0)?.id()?,workspace_id:r.cell(1)?.id()?,document_id:r.cell(2)?.optional(Cell::id)?,task_id:r.cell(3)?.optional(Cell::id)?,parent_id:r.cell(4)?.optional(Cell::id)?,created_by:r.cell(5)?.id()?,body:r.cell(6)?.string()?,resolved_at:r.cell(7)?.optional(Cell::datetime)?,reactions:r.cell(8)?.value()?,created_at:r.cell(9)?.datetime()?,updated_at:r.cell(10)?.datetime()?})).transpose()
}

struct TaskActivityRow {
    id: Uuid,
    actor_user_id: Option<Uuid>,
    channel: String,
    kind: String,
    changes: Value,
    created_at: DateTime<Utc>,
}

async fn load_changes(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, TaskActivityRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, actor_user_id, channel, kind, changes, created_at
        FROM fvoci.task_activity
        WHERE workspace_id = $1 AND task_id = $2 AND id = ANY($3)
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let id: Uuid = row.get("id");
            (
                id,
                TaskActivityRow {
                    id,
                    actor_user_id: row.get("actor_user_id"),
                    channel: row.get("channel"),
                    kind: row.get("kind"),
                    changes: row.get("changes"),
                    created_at: row.get("created_at"),
                },
            )
        })
        .collect())
}

async fn load_comments(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, CommentRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
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
    .bind(ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let comment = row_to_comment(row);
            (comment.id, comment)
        })
        .collect())
}

/// Parent task titles resolved at read time, only when the reader can view
/// the parent's project (direct, group or workspace-role access).
struct ParentTitles {
    project_id: Uuid,
    project_permission: ProjectPermission,
    titles: HashMap<Uuid, Option<String>>,
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
    changes: &Value,
    people: &HashMap<Uuid, ActivityActorOutput>,
    parent_titles: &mut ParentTitles,
) -> Result<Vec<Value>, sqlx::Error> {
    let mut out = Vec::new();
    for change in changes.as_array().into_iter().flatten() {
        let Some(obj) = change.as_object() else {
            continue;
        };
        let field = obj.get("field").and_then(Value::as_str).unwrap_or("");
        let mut map = obj.clone();
        for key in ["from", "to"] {
            let value = obj.get(key).cloned().unwrap_or(Value::Null);
            let visible = visible_value(
                tx,
                workspace_id,
                actor_user_id,
                field,
                value,
                people,
                parent_titles,
            )
            .await?;
            map.insert(key.to_string(), visible);
        }
        out.push(Value::Object(map));
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
    parent_titles: &mut ParentTitles,
) -> Result<Value, sqlx::Error> {
    if let Some(items) = value.as_array() {
        let mapped = items
            .iter()
            .take(50)
            .map(|item| {
                if field != "assigneeIds" {
                    return item.clone();
                }
                let label = item
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|raw| Uuid::parse_str(raw).ok())
                    .and_then(|id| people.get(&id))
                    .map(|person| json!(person.name))
                    .unwrap_or(Value::Null);
                json!({ "id": item.get("id").cloned().unwrap_or(Value::Null), "label": label })
            })
            .collect::<Vec<_>>();
        return Ok(json!({ "items": mapped, "totalCount": items.len() }));
    }
    if field != "parentId" {
        return Ok(value);
    }
    let Some(parent_id) = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
    else {
        return Ok(value);
    };
    if !parent_titles.titles.contains_key(&parent_id) {
        let parent: Option<(Uuid, String)> = sqlx::query_as(
            r#"
            SELECT project_id, title
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut **tx)
        .await?;
        let title = match parent {
            Some((project_id, title)) => {
                let permission = if project_id == parent_titles.project_id {
                    Some(parent_titles.project_permission)
                } else {
                    project_permission_by_id(tx, workspace_id, actor_user_id, project_id).await?
                };
                permission
                    .is_some_and(|permission| permission.at_least(ProjectPermission::View))
                    .then_some(title)
            }
            None => None,
        };
        parent_titles.titles.insert(parent_id, title);
    }
    Ok(json!({
        "id": parent_id.to_string(),
        "label": parent_titles.titles.get(&parent_id).cloned().flatten(),
    }))
}
