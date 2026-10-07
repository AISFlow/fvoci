use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::import_jobs::ImportClaim;
use tokio_util::sync::CancellationToken;

use crate::db::context::{
    begin_read, lock_key_from_uuid, lock_membership_users, recheck_session, session_is_live,
    set_tenant,
};
use crate::db::documents::{between, empty_document_json, DOCUMENT_SCHEMA_VERSION};
use crate::db::holidays::list_holiday_dates;
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::labels::{assignee_filter_member_exists, project_label_exists};
use crate::db::milestones::project_milestone_exists;
use crate::db::projects::{
    load_live_project, lock_project, project_permission, visible_project_sql_for_guest,
    ProjectDbError,
};
use crate::db::task_activity::{record_task_activity, task_activity_snapshot};
use crate::db::view_query::{
    compile_view_query, due_date_sql, prepare_selected_task_view, scalar_value_sql, selected_ids,
    value_column, CompileOptions, CompiledView, CustomPolicy, RootKind, ScalarPolicy,
    SelectedTaskView, SqlArgs, ViewScope,
};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;
use crate::tasks::activity::{patch_activity_fields, ActivitySnapshot};
use crate::tasks::dependency::{
    finish_date, required_dates_present, schedule_ends, violates_inequality, DependencyType,
    ScheduleEnds,
};
use crate::tasks::list_query::{
    cursor_key_for_row, effective_sort_entries, encode_cursor, filter_fingerprint,
    sort_value_token, AssigneeFilter, ParsedTaskListQuery, SortDirection, SortField,
    TaskListCursor, ViewSort,
};

#[derive(Debug, Clone)]
pub struct TaskMetaRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub number: i32,
    pub title: String,
    pub task_type: String,
    pub priority: String,
    pub status_id: Uuid,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate: Option<String>,
    pub parent_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub recurrence: Option<Value>,
    pub sort_key: String,
    pub schema_version: i32,
    pub version: i32,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TaskParentRow {
    pub id: Uuid,
    pub title: String,
    pub task_type: String,
    pub number: i32,
}

#[derive(Debug, Clone)]
pub struct TaskChildRow {
    pub id: Uuid,
    pub number: i32,
    pub title: String,
    pub task_type: String,
    pub status_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskChildProgress {
    pub done: i64,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct TaskDetailRow {
    pub meta: TaskMetaRow,
    pub content_json: Value,
    pub can_edit: bool,
    pub parent: Option<TaskParentRow>,
    pub children: Vec<TaskChildRow>,
    pub child_progress: Option<TaskChildProgress>,
    pub assignee_ids: Vec<Uuid>,
    pub label_ids: Vec<Uuid>,
    pub dependencies: Vec<TaskDependencyEdge>,
}

#[derive(Debug, Clone)]
pub struct TaskDependencyEdge {
    pub blocker_id: Uuid,
    pub blocked_id: Uuid,
    pub dependency_type: String,
    pub lag_days: i32,
}

pub struct TaskListItemRow {
    pub meta: TaskMetaRow,
    pub assignee_ids: Vec<Uuid>,
    pub label_ids: Vec<Uuid>,
}

pub struct TaskListPage {
    pub items: Vec<TaskListItemRow>,
    pub status_counts: Vec<(Uuid, i64)>,
    pub next_cursor: Option<String>,
}

pub struct CreateTaskInput<'a> {
    pub title: &'a str,
    pub task_type: &'a str,
    pub priority: &'a str,
    pub status_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub parent_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub recurrence: Option<Value>,
}

pub(crate) struct TaskChangeRecord<'a> {
    pub(crate) workspace_id: Uuid,
    pub(crate) actor_user_id: Uuid,
    pub(crate) verb: &'a str,
    pub(crate) target_type: &'a str,
    pub(crate) target_id: Uuid,
    pub(crate) payload: Value,
    pub(crate) client_ip: Option<&'a str>,
}

const MAX_TASK_REFS: usize = 50;

fn unique_ids(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    out
}

pub(crate) fn uuid_strings(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

pub(crate) async fn list_task_assignee_ids(
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

pub(crate) async fn list_task_label_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = $2
        ORDER BY label_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub(crate) async fn load_task_refs(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_ids: &[Uuid],
) -> Result<(HashMap<Uuid, Vec<Uuid>>, HashMap<Uuid, Vec<Uuid>>), sqlx::Error> {
    let mut assignees: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut labels: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    if task_ids.is_empty() {
        return Ok((assignees, labels));
    }
    let assignee_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT task_id, user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = ANY($2)
        ORDER BY task_id, user_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    for (task_id, user_id) in assignee_rows {
        assignees.entry(task_id).or_default().push(user_id);
    }
    let label_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT task_id, label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = ANY($2)
        ORDER BY task_id, label_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    for (task_id, label_id) in label_rows {
        labels.entry(task_id).or_default().push(label_id);
    }
    Ok((assignees, labels))
}

async fn membership_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

pub(crate) async fn copy_task_assignees_and_labels(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    source_task_id: Uuid,
    next_task_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_assignees (workspace_id, task_id, user_id)
        SELECT workspace_id, $3, user_id
        FROM fvoci.task_assignees
        WHERE workspace_id = $1 AND task_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(source_task_id)
    .bind(next_task_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_labels (workspace_id, task_id, label_id)
        SELECT workspace_id, $3, label_id
        FROM fvoci.task_labels
        WHERE workspace_id = $1 AND task_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(source_task_id)
    .bind(next_task_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn replace_task_assignees(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    assignee_ids: &[Uuid],
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if assignee_ids.len() > MAX_TASK_REFS {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let unique = unique_ids(assignee_ids);
    // Caller (patch_task_meta) already holds the assignees' membership locks.
    if !unique.is_empty() {
        for user_id in &unique {
            if !membership_exists(tx, workspace_id, *user_id).await? {
                return Ok(Err(ProjectDbError::AssigneeIsNotAMember));
            }
        }
    }
    let current = list_task_assignee_ids(tx, workspace_id, task_id).await?;
    let changed = current.len() != unique.len() || unique.iter().any(|id| !current.contains(id));
    for user_id in &unique {
        if !current.contains(user_id) {
            sqlx::query(
                r#"
                INSERT INTO fvoci.task_assignees (workspace_id, task_id, user_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (task_id, user_id) DO NOTHING
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    for user_id in &current {
        if !unique.contains(user_id) {
            sqlx::query(
                r#"
                DELETE FROM fvoci.task_assignees
                WHERE workspace_id = $1 AND task_id = $2 AND user_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    if changed {
        let added: Vec<Uuid> = unique
            .iter()
            .copied()
            .filter(|id| !current.contains(id))
            .collect();
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "projectId": project_id.to_string(),
                    "assigneeIds": uuid_strings(&unique),
                    "addedAssigneeIds": uuid_strings(&added),
                }),
                client_ip,
            },
        )
        .await?;
    }
    Ok(Ok(()))
}

async fn replace_task_labels(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    label_ids: &[Uuid],
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if label_ids.len() > MAX_TASK_REFS {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let unique = unique_ids(label_ids);
    if !unique.is_empty() {
        let known: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT id
            FROM fvoci.labels
            WHERE workspace_id = $1 AND project_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(&unique)
        .fetch_all(&mut **tx)
        .await?;
        if known.len() != unique.len() {
            return Ok(Err(ProjectDbError::LabelNotFound));
        }
    }
    let current = list_task_label_ids(tx, workspace_id, task_id).await?;
    let changed = current.len() != unique.len() || unique.iter().any(|id| !current.contains(id));
    for label_id in &unique {
        if !current.contains(label_id) {
            sqlx::query(
                r#"
                INSERT INTO fvoci.task_labels (workspace_id, task_id, label_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (task_id, label_id) DO NOTHING
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(label_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    for label_id in &current {
        if !unique.contains(label_id) {
            sqlx::query(
                r#"
                DELETE FROM fvoci.task_labels
                WHERE workspace_id = $1 AND task_id = $2 AND label_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .bind(label_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    if changed {
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "projectId": project_id.to_string(),
                    "labelIds": uuid_strings(&unique),
                }),
                client_ip,
            },
        )
        .await?;
    }
    Ok(Ok(()))
}

pub(crate) async fn record_task_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: TaskChangeRecord<'_>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload,
            ip: change.client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

async fn default_backlog_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2 AND category = 'backlog'
        ORDER BY sort_key COLLATE "C"
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    if row.is_some() {
        return Ok(row.map(|(id,)| id));
    }
    let fallback: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C"
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(fallback.map(|(id,)| id))
}

/// Serializes WIP-limit checks per target status (transaction-scoped).
pub(crate) const TASK_STATUS_LOCK_NAMESPACE: i32 = 1_907_003;

fn violates_task_hierarchy(child_type: &str, parent_type: &str) -> bool {
    if child_type == "subtask" {
        !matches!(parent_type, "task" | "bug" | "story")
    } else if child_type == "epic" {
        true
    } else {
        parent_type != "epic"
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod selected_task_detail_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::lookup::selected_lookup_tests::{origin_task, project, session};
    use crate::db::task_origins::{
        create_document_task_backend, origin_request_hash, DocumentTaskRequest,
    };

    pub(crate) async fn setup() -> (Fixture, Uuid, String, Uuid, Uuid) {
        let f = Fixture::new().await;
        let (credential, cookie) = session(&f, f.user).await;
        let project = project(&f, credential).await;
        let task = origin_task(&f, credential, project, Uuid::now_v7())
            .await
            .task_id();
        (f, credential, cookie, project, task)
    }

    async fn create(
        f: &Fixture,
        credential: Uuid,
        project: Uuid,
        input: CreateTaskInput<'_>,
    ) -> Uuid {
        let dto = crate::api::dto::CreateTaskBody {
            title: input.title.into(),
            task_type: input.task_type.into(),
            priority: input.priority.into(),
            status_id: input.status_id,
            start_date: input.start_date,
            due_date: input.due_date,
            parent_id: input.parent_id,
            milestone_id: input.milestone_id,
            recurrence: input.recurrence.clone(),
        };
        let hash = origin_request_hash(
            f.user,
            project,
            None,
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
                request_id: Uuid::now_v7(),
                anchor: None,
                request_hash: &hash,
                task: input,
                self_assign: false,
            },
            None,
            "api",
        )
        .await
        .unwrap()
        .unwrap()
        .task_id()
    }

    fn input<'a>(title: &'a str, task_type: &'a str, parent: Option<Uuid>) -> CreateTaskInput<'a> {
        CreateTaskInput {
            title,
            task_type,
            priority: "none",
            status_id: None,
            start_date: None,
            due_date: None,
            parent_id: parent,
            milestone_id: None,
            recurrence: None,
        }
    }

    async fn read(f: &Fixture, credential: Uuid, task: Uuid) -> TaskDetailRow {
        get_task_backend(&f.backend, f.workspace, task, f.user, credential)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn wiki_aux_task_detail_origin_lookup_new_client_literal_body_and_personal_read() {
        let (f, credential, _, project, task) = setup().await;
        let lookup = crate::db::lookup::lookup_display_id_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            "ORIGIN-2",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(lookup.len(), 1);
        assert_eq!(lookup[0].id, task);
        let client = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let detail = get_task_backend(
            &Backend::Sqlite(client.clone()),
            f.workspace,
            task,
            f.user,
            credential,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(detail.meta.id, task);
        assert_eq!(detail.meta.workspace_id, f.workspace);
        assert_eq!(detail.meta.project_id, project);
        assert_eq!(detail.meta.number, 2);
        assert_eq!(detail.meta.title, "실제 원본 작업 中 😀");
        assert_eq!(detail.meta.created_by, f.user);
        assert_eq!(detail.meta.schema_version, DOCUMENT_SCHEMA_VERSION);
        assert_eq!(detail.meta.version, 1);
        assert_eq!(detail.content_json, empty_document_json());
        assert!(detail.can_edit);
        assert!(detail.parent.is_none() && detail.children.is_empty());
        let progress = detail.child_progress.unwrap();
        assert_eq!((progress.done, progress.total), (0, 0));
        assert!(
            detail.assignee_ids.is_empty()
                && detail.label_ids.is_empty()
                && detail.dependencies.is_empty()
        );
        // Projection fixture, not a native engine/body-save substitute. The
        // normal/current Rust native writer and browser mount run separately.
        let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"현재 본문 中 😀"}]}]});
        sqlx::query("UPDATE tasks SET content_json=?1,version=2 WHERE id=?2")
            .bind(body.to_string())
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET kind='personal' WHERE id=?1")
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
        let personal = get_task_backend(
            &Backend::Sqlite(client.clone()),
            f.workspace,
            task,
            f.user,
            credential,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(personal.meta.id, task);
        assert_eq!(personal.meta.project_id, project);
        assert_eq!(personal.meta.version, 2);
        assert_eq!(personal.content_json, body);
        assert!(personal.can_edit);
        let origins = crate::db::task_origins::list_document_task_origins_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            50,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(origins.count, 1);
        assert_eq!(origins.items[0].document_id, f.document);
        assert_eq!(origins.items[0].task_id, task);
        client.close().await;
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_task_detail_all_fields_refs_hierarchy_order_and_lifecycle() {
        let f = Fixture::new().await;
        let credential = session(&f, f.user).await.0;
        let project = project(&f, credential).await;
        let epic = create(&f, credential, project, input("상위 中 😀", "epic", None)).await;
        let mut target = input("현재 전체 필드", "task", Some(epic));
        target.priority = "high";
        target.start_date = Some(NaiveDate::from_ymd_opt(2026, 10, 5).unwrap());
        target.due_date = Some(NaiveDate::from_ymd_opt(2026, 10, 6).unwrap());
        target.recurrence = Some(json!({"kind":"daily"}));
        let task = create(&f, credential, project, target).await;
        let open = create(
            &f,
            credential,
            project,
            input("Open", "subtask", Some(task)),
        )
        .await;
        let done = create(
            &f,
            credential,
            project,
            input("Done", "subtask", Some(task)),
        )
        .await;
        let canceled = create(
            &f,
            credential,
            project,
            input("Canceled", "subtask", Some(task)),
        )
        .await;
        let archived = create(
            &f,
            credential,
            project,
            input("Archived", "subtask", Some(task)),
        )
        .await;
        let deleted = create(
            &f,
            credential,
            project,
            input("Deleted", "subtask", Some(task)),
        )
        .await;
        let before = create(&f, credential, project, input("Incoming", "task", None)).await;
        let after = create(&f, credential, project, input("Outgoing", "task", None)).await;
        for (child, category) in [(done, "done"), (canceled, "canceled")] {
            sqlx::query("UPDATE tasks SET status_id=(SELECT id FROM statuses WHERE project_id=?1 AND category=?2 LIMIT 1) WHERE id=?3")
                .bind(project.as_bytes().as_slice()).bind(category).bind(child.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        }
        sqlx::query("UPDATE tasks SET archived_at=1000000 WHERE id=?1")
            .bind(archived.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET deleted_at=1000000 WHERE id=?1")
            .bind(deleted.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // Tie timestamps deliberately; UUID DESC must be the second key.
        sqlx::query("UPDATE tasks SET created_at=2000000 WHERE parent_id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let milestone = Uuid::now_v7();
        sqlx::query("INSERT INTO milestones(id,workspace_id,project_id,name,sort_key) VALUES(?1,?2,?3,'Milestone','M')")
            .bind(milestone.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"확인할 본문 😀"}]}]});
        sqlx::query("UPDATE tasks SET estimate='1.2500',milestone_id=?1,sort_key='M',version=17,created_at=3000001,updated_at=4000002,content_json=?2 WHERE id=?3")
            .bind(milestone.as_bytes().as_slice()).bind(body.to_string()).bind(task.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut users = vec![f.user];
        for number in [20u128, 10] {
            let user = Uuid::from_u128(number);
            sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Assignee')")
                .bind(user.as_bytes().as_slice())
                .bind(format!("{user}@example.test"))
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
            users.push(user);
        }
        for user in &users {
            sqlx::query(
                "INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(task.as_bytes().as_slice())
            .bind(user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let mut labels = vec![Uuid::from_u128(40), Uuid::from_u128(30)];
        for label in &labels {
            sqlx::query("INSERT INTO labels(id,workspace_id,project_id,name,color) VALUES(?1,?2,?3,?4,'red')")
                .bind(label.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(label.to_string()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO task_labels(workspace_id,task_id,label_id) VALUES(?1,?2,?3)")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(task.as_bytes().as_slice())
                .bind(label.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        for (blocker, blocked, kind, lag) in [(before, task, "FS", 2i32), (task, after, "SS", 1)] {
            sqlx::query("INSERT INTO task_dependencies(workspace_id,blocker_id,blocked_id,type,lag_days) VALUES(?1,?2,?3,?4,?5)")
                .bind(f.workspace.as_bytes().as_slice()).bind(blocker.as_bytes().as_slice()).bind(blocked.as_bytes().as_slice()).bind(kind).bind(lag).execute(&f.pool).await.unwrap();
        }
        let detail = read(&f, credential, task).await;
        let meta = &detail.meta;
        assert_eq!(
            (meta.id, meta.workspace_id, meta.project_id, meta.number),
            (task, f.workspace, project, 3)
        );
        assert_eq!(
            (
                meta.title.as_str(),
                meta.task_type.as_str(),
                meta.priority.as_str()
            ),
            ("현재 전체 필드", "task", "high")
        );
        let status: Vec<u8> = sqlx::query_scalar("SELECT status_id FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(meta.status_id, Uuid::from_slice(&status).unwrap());
        assert_eq!(meta.start_date, NaiveDate::from_ymd_opt(2026, 10, 5));
        assert_eq!(meta.due_date, NaiveDate::from_ymd_opt(2026, 10, 6));
        assert!(meta.due_at.is_none());
        assert_eq!(meta.estimate.as_deref(), Some("1.2500"));
        assert_eq!(meta.parent_id, Some(epic));
        assert_eq!(meta.milestone_id, Some(milestone));
        assert_eq!(meta.recurrence, Some(json!({"kind":"daily"})));
        assert_eq!(meta.sort_key, "M");
        assert_eq!(
            (meta.schema_version, meta.version),
            (DOCUMENT_SCHEMA_VERSION, 17)
        );
        assert!(meta.archived_at.is_none());
        assert_eq!(meta.created_by, f.user);
        assert_eq!(meta.created_at.timestamp_micros(), 3000001);
        assert_eq!(meta.updated_at.timestamp_micros(), 4000002);
        assert_eq!(detail.content_json, body);
        assert!(detail.can_edit);
        let parent = detail.parent.unwrap();
        assert_eq!(
            (
                parent.id,
                parent.title.as_str(),
                parent.task_type.as_str(),
                parent.number
            ),
            (epic, "상위 中 😀", "epic", 2)
        );
        let mut expected_children = vec![open, done, canceled];
        expected_children.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(
            detail
                .children
                .iter()
                .map(|child| child.id)
                .collect::<Vec<_>>(),
            expected_children
        );
        assert!(detail
            .children
            .iter()
            .all(|child| child.task_type == "subtask"));
        let progress = detail.child_progress.unwrap();
        assert_eq!((progress.done, progress.total), (1, 2));
        users.sort_unstable();
        labels.sort_unstable();
        assert_eq!(detail.assignee_ids, users);
        assert_eq!(detail.label_ids, labels);
        let mut edges = vec![
            (before, task, "FS".to_owned(), 2),
            (task, after, "SS".to_owned(), 1),
        ];
        edges.sort_unstable_by_key(|edge| (edge.0, edge.1));
        assert_eq!(
            detail
                .dependencies
                .iter()
                .map(|edge| (
                    edge.blocker_id,
                    edge.blocked_id,
                    edge.dependency_type.clone(),
                    edge.lag_days
                ))
                .collect::<Vec<_>>(),
            edges
        );
        let child = read(&f, credential, open).await;
        assert_eq!(child.parent.unwrap().id, task);
        assert!(child.child_progress.is_none());
        sqlx::query("UPDATE tasks SET archived_at=5000003 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let archived_detail = read(&f, credential, task).await;
        assert_eq!(
            archived_detail.meta.archived_at.unwrap().timestamp_micros(),
            5000003
        );
        assert!(
            archived_detail.can_edit,
            "metadata permission stays independent from archive UI policy"
        );
        sqlx::query("UPDATE tasks SET deleted_at=1 WHERE id=?1")
            .bind(epic.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(read(&f, credential, task).await.parent.is_none());
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_task_detail_current_credentials_grants_affiliation_and_tenant_denials() {
        let (f, credential, _, project, task) = setup().await;
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other')")
            .bind(other.as_bytes().as_slice())
            .bind(format!("{other}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other_credential = session(&f, other).await.0;
        let read_other =
            || get_task_backend(&f.backend, f.workspace, task, other, other_credential);
        assert!(matches!(
            read_other().await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Detail grant')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let viewer = read_other().await.unwrap().unwrap();
        assert_eq!(viewer.meta.id, task);
        assert!(!viewer.can_edit);
        let mut revoke = f.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query(
            "DELETE FROM group_members WHERE workspace_id=?1 AND group_id=?2 AND user_id=?3",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(group.as_bytes().as_slice())
        .bind(other.as_bytes().as_slice())
        .execute(&mut *revoke)
        .await
        .unwrap();
        revoke.commit().await.unwrap();
        assert!(matches!(
            read_other().await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'member')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(other.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(read_other().await.unwrap().unwrap().can_edit);
        assert!(matches!(
            get_task_backend(&f.backend, f.workspace, task, other, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        assert!(matches!(
            get_task_backend(&f.backend, Uuid::now_v7(), task, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        for (table, column) in [
            ("sessions", "revoked_at"),
            ("users", "suspended_at"),
            ("users", "deleted_at"),
            ("workspaces", "deleted_at"),
            ("projects", "deleted_at"),
            ("tasks", "deleted_at"),
        ] {
            let id = match table {
                "sessions" => credential,
                "users" => f.user,
                "workspaces" => f.workspace,
                "projects" => project,
                _ => task,
            };
            let sql = format!("UPDATE {table} SET {column}=1 WHERE id=?1");
            sqlx::query(&sql)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let denied = get_task_backend(&f.backend, f.workspace, task, f.user, credential)
                .await
                .unwrap();
            if table == "sessions" || table == "users" {
                assert!(matches!(denied, Err(ProjectDbError::Forbidden)));
            } else {
                assert!(matches!(denied, Err(ProjectDbError::NotFound)));
            }
            let sql = format!("UPDATE {table} SET {column}=NULL WHERE id=?1");
            sqlx::query(&sql)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(read(&f, credential, task).await.meta.id, task);
        }
        let target = crate::db::projects::create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            crate::db::projects::CreateProjectInput {
                key: "MOVED",
                name: "Moved",
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
        .id;
        let status: Vec<u8> = sqlx::query_scalar(
            "SELECT id FROM statuses WHERE project_id=?1 AND category='backlog' LIMIT 1",
        )
        .bind(target.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE tasks SET project_id=?1,status_id=?2 WHERE id=?3")
            .bind(target.as_bytes().as_slice())
            .bind(&status)
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            matches!(read_other().await.unwrap(), Err(ProjectDbError::NotFound)),
            "old project grant cannot authorize current affiliation"
        );
        let current = read(&f, credential, task).await;
        assert_eq!(current.meta.project_id, target);
        assert_eq!(current.meta.id, task);
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            read_other().await.unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_task_detail_strict_decode_original_error_rollback_then_healthy_read() {
        let (f, credential, _, _, task) = setup().await;
        sqlx::query("UPDATE tasks SET due_at=?1 WHERE id=?2")
            .bind(i64::MAX)
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let error = get_task_backend(&f.backend, f.workspace, task, f.user, credential)
            .await
            .unwrap_err();
        assert!(
            matches!(error,sqlx::Error::Protocol(ref message) if message=="SQLite instant out of range")
        );
        // Actual returned local rollback, followed by a local writer; this is
        // never proof of original remote-stream settlement or native replay.
        let mut repair = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(tx) = repair.operation() else {
            panic!("actual local fixture")
        };
        tx.set_tenant(f.workspace).unwrap();
        tx.execute(
            "UPDATE tasks SET due_at=NULL WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(f.workspace), Cell::uuid(task)],
        )
        .await
        .unwrap();
        repair.commit().await.unwrap();
        let healthy = read(&f, credential, task).await;
        assert_eq!(healthy.meta.id, task);
        assert!(healthy.meta.due_at.is_none());
        assert_eq!(healthy.content_json, empty_document_json());
        let foreign = Uuid::now_v7();
        let result = sqlx::query(
            "INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3)",
        )
        .bind(foreign.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&f.pool)
        .await;
        let constraint = result.unwrap_err();
        assert!(
            constraint
                .as_database_error()
                .is_some_and(|error| error.is_foreign_key_violation()),
            "real FK1 rejects wrong tenant ref; never disable constraint"
        );
        assert!(read(&f, credential, task).await.assignee_ids.is_empty());
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

fn allowed_child_types(task_type: &str) -> &'static [&'static str] {
    match task_type {
        "epic" => &["task", "bug", "story"],
        "subtask" => &[],
        _ => &["subtask"],
    }
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    use chrono::Datelike;
    let (next_year, next_month) = if month == 12 {
        (year.checked_add(1)?, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)?
        .pred_opt()
        .map(|d| d.day())
}

/// Matches source `shiftDate` monthly rollover (e.g. 2026-01-31 → 2026-03-03).
fn shift_recurrence_date_monthly(date: NaiveDate) -> Option<NaiveDate> {
    use chrono::Datelike;
    let (new_year, new_month) = if date.month() == 12 {
        (date.year().checked_add(1)?, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let days_in_target = days_in_month(new_year, new_month)?;
    if date.day() <= days_in_target {
        return NaiveDate::from_ymd_opt(new_year, new_month, date.day());
    }
    let (overflow_year, overflow_month) = if new_month == 12 {
        (new_year.checked_add(1)?, 1)
    } else {
        (new_year, new_month + 1)
    };
    NaiveDate::from_ymd_opt(overflow_year, overflow_month, date.day() - days_in_target)
}

/// Returns None when the shifted date leaves the four-digit-year range the API
/// accepts (the source fails the request in that case too).
fn shift_recurrence_date(date: NaiveDate, kind: &str) -> Option<NaiveDate> {
    use chrono::Datelike;
    let shifted = match kind {
        "daily" => date.checked_add_signed(chrono::Duration::days(1)),
        "weekly" => date.checked_add_signed(chrono::Duration::days(7)),
        "monthly" => shift_recurrence_date_monthly(date),
        _ => Some(date),
    }?;
    (shifted.year() <= 9999).then_some(shifted)
}

fn parse_recurrence_kind(value: &Value) -> Option<&str> {
    value
        .as_object()
        .and_then(|obj| obj.get("kind"))
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "daily" | "weekly" | "monthly"))
}

async fn has_children_outside_types(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    allowed: &[&str],
) -> Result<bool, sqlx::Error> {
    let row: (bool,) = if allowed.is_empty() {
        sqlx::query_as(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM fvoci.tasks
                WHERE workspace_id = $1 AND parent_id = $2
            )
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM fvoci.tasks
                WHERE workspace_id = $1 AND parent_id = $2
                  AND type <> ALL($3::text[])
            )
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(allowed)
        .fetch_one(&mut **tx)
        .await?
    };
    Ok(row.0)
}

async fn assert_task_hierarchy(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    next_type: &str,
    next_parent: Option<Uuid>,
    parent_changed: bool,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if let Some(parent_id) = next_parent {
        if parent_id == task_id {
            return Ok(Err(ProjectDbError::Conflict));
        }
        type ParentRow = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>, String);
        let parent: Option<ParentRow> = sqlx::query_as(
            r#"
                SELECT project_id, deleted_at, archived_at, type
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2
                "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((parent_project, deleted, archived, parent_type)) = parent else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        if deleted.is_some() || parent_project != project_id {
            return Ok(Err(ProjectDbError::NotFound));
        }
        if parent_changed && archived.is_some() {
            return Ok(Err(ProjectDbError::TaskArchived));
        }
        if violates_task_hierarchy(next_type, &parent_type) {
            return Ok(Err(ProjectDbError::Conflict));
        }
    } else if next_type == "subtask" {
        return Ok(Err(ProjectDbError::Conflict));
    }
    if has_children_outside_types(tx, workspace_id, task_id, allowed_child_types(next_type)).await?
    {
        return Ok(Err(ProjectDbError::Conflict));
    }
    Ok(Ok(()))
}

async fn lock_status_for_transition(
    tx: &mut Transaction<'_, Postgres>,
    status_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TASK_STATUS_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(status_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn count_active_tasks_in_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*)
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND status_id = $2
          AND deleted_at IS NULL
          AND archived_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(status_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

struct StatusInfo {
    category: String,
    wip_limit: Option<i32>,
}

async fn load_status_info(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    status_id: Uuid,
) -> Result<Option<StatusInfo>, sqlx::Error> {
    let row: Option<(String, Option<i32>)> = sqlx::query_as(
        r#"
        SELECT category, wip_limit
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(category, wip_limit)| StatusInfo {
        category,
        wip_limit,
    }))
}

#[derive(Debug, Clone)]
pub(crate) struct TaskRowRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub number: i32,
    pub title: String,
    pub task_type: String,
    pub priority: String,
    pub status_id: Uuid,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub estimate: Option<String>,
    pub parent_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub sort_key: String,
    pub schema_version: i32,
    pub version: i32,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) fn map_task_row(row: &sqlx::postgres::PgRow) -> Result<TaskRowRecord, sqlx::Error> {
    Ok(TaskRowRecord {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        number: row.try_get("number")?,
        title: row.try_get("title")?,
        task_type: row.try_get("task_type")?,
        priority: row.try_get("priority")?,
        status_id: row.try_get("status_id")?,
        start_date: row.try_get("start_date")?,
        due_date: row.try_get("due_date")?,
        due_at: row.try_get("due_at")?,
        estimate: row.try_get("estimate")?,
        parent_id: row.try_get("parent_id")?,
        milestone_id: row.try_get("milestone_id")?,
        sort_key: row.try_get("sort_key")?,
        schema_version: row.try_get("schema_version")?,
        version: row.try_get("version")?,
        archived_at: row.try_get("archived_at")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(crate) fn row_to_meta(
    workspace_id: Uuid,
    row: TaskRowRecord,
    recurrence: Option<Value>,
) -> TaskMetaRow {
    TaskMetaRow {
        id: row.id,
        workspace_id,
        project_id: row.project_id,
        number: row.number,
        title: row.title,
        task_type: row.task_type,
        priority: row.priority,
        status_id: row.status_id,
        start_date: row.start_date,
        due_date: row.due_date,
        due_at: row.due_at,
        estimate: row.estimate,
        parent_id: row.parent_id,
        milestone_id: row.milestone_id,
        sort_key: row.sort_key,
        schema_version: row.schema_version,
        version: row.version,
        recurrence,
        archived_at: row.archived_at,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

pub(crate) async fn load_task_recurrence(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Value>, sqlx::Error> {
    sqlx::query_scalar("SELECT recurrence FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(&mut **tx)
        .await
}

async fn load_task_parent(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    parent_id: Uuid,
) -> Result<Option<TaskParentRow>, sqlx::Error> {
    let row: Option<(Uuid, String, String, i32)> = sqlx::query_as(
        r#"
        SELECT id, title, type, number
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND project_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id, title, task_type, number)| TaskParentRow {
        id,
        title,
        task_type,
        number,
    }))
}

async fn load_task_children(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<Vec<TaskChildRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, i32, String, String, Uuid)>(
        r#"
        SELECT id, number, title, type, status_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND parent_id = $2
          AND deleted_at IS NULL AND archived_at IS NULL
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, number, title, task_type, status_id)| TaskChildRow {
            id,
            number,
            title,
            task_type,
            status_id,
        })
        .collect())
}

async fn load_task_child_progress(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    parent_id: Uuid,
) -> Result<TaskChildProgress, sqlx::Error> {
    let row: (i64, i64) = sqlx::query_as(
        r#"
        SELECT
            count(*) FILTER (WHERE s.category = 'done')::bigint,
            count(*)::bigint
        FROM fvoci.tasks t
        INNER JOIN fvoci.statuses s
          ON s.workspace_id = t.workspace_id AND s.id = t.status_id
        WHERE t.workspace_id = $1 AND t.parent_id = $2
          AND t.deleted_at IS NULL AND t.archived_at IS NULL
          AND s.category <> 'canceled'
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(TaskChildProgress {
        done: row.0,
        total: row.1,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn create_task(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateTaskInput<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let created = create_task_tx(
        &mut tx,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
        channel,
    )
    .await?;
    match created {
        Ok(row) => {
            tx.commit().await?;
            Ok(Ok(row))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

/// `create_task` inside the caller's tenant transaction (import runs append
/// their fenced ref in the same transaction). The caller rolls back on `Err`.
#[allow(clippy::too_many_arguments)]
pub async fn create_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateTaskInput<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(ProjectDbError::NotFound));
    }

    insert_task_in_locked_project(
        tx,
        workspace_id,
        project_id,
        actor_user_id,
        &input,
        client_ip,
        channel,
    )
    .await
}

/// Inserts a task into a project the caller has already locked and authorized
/// for edit (active session, live workspace, writable project) in this
/// transaction. Records the create event, audit and activity.
pub(crate) async fn insert_task_in_locked_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    input: &CreateTaskInput<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    let task_id = Uuid::now_v7();
    if input.task_type == "subtask" && input.parent_id.is_none() {
        return Ok(Err(ProjectDbError::Conflict));
    }

    if let Some(parent_id) = input.parent_id {
        let parent: Option<(Uuid, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            r#"
            SELECT project_id, deleted_at, type
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((parent_project, deleted, parent_type)) = parent else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        if deleted.is_some() || parent_project != project_id {
            return Ok(Err(ProjectDbError::NotFound));
        }
        if violates_task_hierarchy(input.task_type, &parent_type) {
            return Ok(Err(ProjectDbError::Conflict));
        }
    }

    if let Some(milestone_id) = input.milestone_id {
        if !project_milestone_exists(tx, workspace_id, project_id, milestone_id).await? {
            return Ok(Err(ProjectDbError::MilestoneNotFound));
        }
    }

    let status_id = if let Some(status_id) = input.status_id {
        let valid: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT id FROM fvoci.statuses
            WHERE workspace_id = $1 AND project_id = $2 AND id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(status_id)
        .fetch_optional(&mut **tx)
        .await?;
        if valid.is_none() {
            return Ok(Err(ProjectDbError::StatusNotInWorkflow));
        }
        status_id
    } else {
        let Some(status_id) = default_backlog_status(tx, workspace_id, project_id).await? else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        status_id
    };

    let number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;

    let last_sort: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT sort_key
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND project_id = $2
          AND status_id = $3
          AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C" DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .fetch_optional(&mut **tx)
    .await?;
    let sort_key = match between(last_sort.as_ref().map(|(key,)| key.as_str()), None) {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("task sort_key allocation failed: {err}");
            return Ok(Err(ProjectDbError::Conflict));
        }
    };

    let row = map_task_row(
        &sqlx::query(
            r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            start_date, due_date, parent_id, milestone_id, recurrence, sort_key,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
        )
        RETURNING id, project_id, number, title, type AS task_type, priority, status_id,
                  start_date, due_date, due_at, estimate::text AS estimate, parent_id,
                  milestone_id, sort_key, schema_version, version, archived_at, created_by,
                  created_at, updated_at
        "#,
        )
        .bind(task_id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(number.0)
        .bind(input.title.trim())
        .bind(input.task_type)
        .bind(input.priority)
        .bind(status_id)
        .bind(input.start_date)
        .bind(input.due_date)
        .bind(input.parent_id)
        .bind(input.milestone_id)
        .bind(input.recurrence.clone())
        .bind(&sort_key)
        .bind(DOCUMENT_SCHEMA_VERSION)
        .bind(empty_document_json())
        .bind(actor_user_id)
        .fetch_one(&mut **tx)
        .await?,
    )?;

    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.created",
            target_type: "task",
            target_id: task_id,
            payload: json!({
                "taskId": task_id.to_string(),
                "projectId": project_id.to_string(),
                "title": input.title.trim(),
            }),
            client_ip,
        },
    )
    .await?;
    record_task_activity(
        tx,
        workspace_id,
        task_id,
        actor_user_id,
        channel,
        None,
        &ActivitySnapshot::new(),
    )
    .await?;

    Ok(Ok(row_to_meta(workspace_id, row, input.recurrence.clone())))
}

pub async fn get_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<TaskDetailRow, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(raw) = sqlx::query(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate::text AS estimate,
               t.parent_id, t.milestone_id, t.sort_key, t.schema_version, t.version,
               t.archived_at, t.created_by, t.created_at, t.updated_at
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1 AND t.id = $2 AND t.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let task_row = map_task_row(&raw)?;
    let content_json: Value = sqlx::query_scalar(
        "SELECT content_json FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;
    let project_id = task_row.project_id;
    let project = load_live_project(&mut tx, workspace_id, project_id).await?;
    let Some(project) = project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let permission = project_permission(&mut tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    let meta = row_to_meta(
        workspace_id,
        task_row,
        load_task_recurrence(&mut tx, workspace_id, task_id).await?,
    );
    let parent = if let Some(parent_id) = meta.parent_id {
        load_task_parent(&mut tx, workspace_id, project_id, parent_id).await?
    } else {
        None
    };
    let children = load_task_children(&mut tx, workspace_id, task_id).await?;
    let child_progress = if meta.task_type == "subtask" {
        None
    } else {
        Some(load_task_child_progress(&mut tx, workspace_id, task_id).await?)
    };
    let assignee_ids = list_task_assignee_ids(&mut tx, workspace_id, task_id).await?;
    let label_ids = list_task_label_ids(&mut tx, workspace_id, task_id).await?;
    let dependencies = list_task_dependency_edges(&mut tx, workspace_id, task_id).await?;

    tx.commit().await?;
    Ok(Ok(TaskDetailRow {
        meta,
        content_json,
        can_edit: permission.at_least(ProjectPermission::Edit),
        parent,
        children,
        child_progress,
        assignee_ids,
        label_ids,
        dependencies,
    }))
}

/// Selected reader for the existing detail DTO. PostgreSQL retains its exact
/// repeatable-read consumer; family credentials, affiliation, grants and every
/// returned field share one read transaction.
pub async fn get_task_backend(
    backend: &Backend,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<TaskDetailRow, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_task(pool, workspace_id, task_id, actor_user_id, session_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = match &mut tx {
        DbTx::SqliteFamily(family) => {
            get_task_family(family, workspace_id, task_id, actor_user_id, session_id).await
        }
        DbTx::Postgres(_) => Err(sqlx::Error::Protocol(
            "family task detail requires selected family transaction".into(),
        )),
    };
    match result {
        Ok(Ok(detail)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(detail))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(TaskDetailReadRefusal(refusal))),
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
#[error("task detail read refused: {0:?}")]
struct TaskDetailReadRefusal(ProjectDbError);

fn map_task_detail_family_row(row: &FamilyRow) -> Result<TaskRowRecord, sqlx::Error> {
    Ok(TaskRowRecord {
        id: row.cell(0)?.id()?,
        project_id: row.cell(1)?.id()?,
        number: row.cell(2)?.int32()?,
        title: row.cell(3)?.string()?,
        task_type: row.cell(4)?.string()?,
        priority: row.cell(5)?.string()?,
        status_id: row.cell(6)?.id()?,
        start_date: row.cell(7)?.optional(Cell::date)?,
        due_date: row.cell(8)?.optional(Cell::date)?,
        due_at: row.cell(9)?.optional(Cell::datetime)?,
        estimate: row.cell(10)?.optional(Cell::string)?,
        parent_id: row.cell(11)?.optional(Cell::id)?,
        milestone_id: row.cell(12)?.optional(Cell::id)?,
        sort_key: row.cell(13)?.string()?,
        schema_version: row.cell(14)?.int32()?,
        version: row.cell(15)?.int32()?,
        archived_at: row.cell(16)?.optional(Cell::datetime)?,
        created_by: row.cell(17)?.id()?,
        created_at: row.cell(18)?.datetime()?,
        updated_at: row.cell(19)?.datetime()?,
    })
}

async fn get_task_family(
    tx: &mut FamilyTx,
    workspace: Uuid,
    task: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<TaskDetailRow, ProjectDbError>, sqlx::Error> {
    let mut op = OperationTx::SqliteFamily(&mut *tx);
    op.set_tenant(workspace).await?;
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let args = [Cell::uuid(workspace), Cell::uuid(task)];
    let rows = tx.query(
        "SELECT id,project_id,number,title,type,priority,status_id,start_date,due_date,due_at,estimate,parent_id,milestone_id,sort_key,schema_version,version,archived_at,created_by,created_at,updated_at,content_json,recurrence FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
        &args,
    ).await?;
    let Some(row) = rows.first() else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let record = map_task_detail_family_row(row)?;
    let project = record.project_id;
    let Some(permission) = OperationTx::SqliteFamily(&mut *tx)
        .project_permission_by_id(workspace, actor, project)
        .await?
        .filter(|permission| permission.at_least(ProjectPermission::View))
    else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let meta = row_to_meta(workspace, record, row.cell(21)?.optional(Cell::value)?);
    let content_json = row.cell(20)?.value()?;
    let parent = if let Some(parent) = meta.parent_id {
        tx.query(
            "SELECT id,title,type,number FROM tasks WHERE workspace_id=?1 AND id=?2 AND project_id=?3 AND deleted_at IS NULL",
            &[Cell::uuid(workspace), Cell::uuid(parent), Cell::uuid(project)],
        ).await?.first().map(|row| -> Result<TaskParentRow, sqlx::Error> {
            Ok(TaskParentRow {
                id: row.cell(0)?.id()?,
                title: row.cell(1)?.string()?,
                task_type: row.cell(2)?.string()?,
                number: row.cell(3)?.int32()?,
            })
        }).transpose()?
    } else {
        None
    };
    let children = tx.query(
        "SELECT id,number,title,type,status_id FROM tasks WHERE workspace_id=?1 AND parent_id=?2 AND deleted_at IS NULL AND archived_at IS NULL ORDER BY created_at DESC,id DESC",
        &args,
    ).await?.iter().map(|row| {
        Ok(TaskChildRow {
            id: row.cell(0)?.id()?,
            number: row.cell(1)?.int32()?,
            title: row.cell(2)?.string()?,
            task_type: row.cell(3)?.string()?,
            status_id: row.cell(4)?.id()?,
        })
    }).collect::<Result<Vec<_>,sqlx::Error>>()?;
    let child_progress = if meta.task_type == "subtask" {
        None
    } else {
        let rows = tx.query(
            "SELECT count(*) FILTER (WHERE s.category='done'),count(*) FROM tasks t INNER JOIN statuses s ON s.workspace_id=t.workspace_id AND s.id=t.status_id WHERE t.workspace_id=?1 AND t.parent_id=?2 AND t.deleted_at IS NULL AND t.archived_at IS NULL AND s.category<>'canceled'",
            &args,
        ).await?;
        let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
        Some(TaskChildProgress {
            done: row.cell(0)?.integer()?,
            total: row.cell(1)?.integer()?,
        })
    };
    let assignee_ids = tx.query(
        "SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 ORDER BY user_id",
        &args,
    ).await?.iter().map(|row| row.cell(0)?.id()).collect::<Result<Vec<_>,sqlx::Error>>()?;
    let label_ids = tx.query(
        "SELECT label_id FROM task_labels WHERE workspace_id=?1 AND task_id=?2 ORDER BY label_id",
        &args,
    ).await?.iter().map(|row| row.cell(0)?.id()).collect::<Result<Vec<_>,sqlx::Error>>()?;
    let dependencies = tx.query(
        "SELECT blocker_id,blocked_id,type,lag_days FROM task_dependencies WHERE workspace_id=?1 AND (blocker_id=?2 OR blocked_id=?2) ORDER BY blocker_id,blocked_id",
        &args,
    ).await?.iter().map(|row| {
        Ok(map_dependency_row(row.cell(0)?.id()?,row.cell(1)?.id()?,row.cell(2)?.string()?,row.cell(3)?.int32()?))
    }).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Ok(TaskDetailRow {
        meta,
        content_json,
        can_edit: permission.at_least(ProjectPermission::Edit),
        parent,
        children,
        child_progress,
        assignee_ids,
        label_ids,
        dependencies,
    }))
}

type TaskListCursorAnchor = (
    DateTime<Utc>,
    DateTime<Utc>,
    Uuid,
    i32,
    String,
    String,
    String,
    Uuid,
    Option<NaiveDate>,
    String,
);

pub async fn list_project_tasks(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    query: &ParsedTaskListQuery,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    list_tasks_in_scope(
        pool,
        workspace_id,
        Some(project_id),
        actor_user_id,
        session_id,
        query,
    )
    .await
}

/// The selected project route uses the original PostgreSQL reader, or one
/// authorized family snapshot. No read response is returned before release is
/// acknowledged; cleanup uncertainty retains the original refusal/error.
pub async fn list_project_tasks_backend(
    backend: &Backend,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
    query: &ParsedTaskListQuery,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    list_project_tasks_backend_with_use(
        backend,
        workspace,
        project,
        actor,
        credential,
        query,
        TaskScalarUse::Production,
    )
    .await
}

#[derive(Clone, Copy)]
enum TaskScalarUse {
    Production,
    // Bootstrap only in the DB-test binary: never an endpoint/env fallback.
    #[cfg(all(test, feature = "db-tests"))]
    ReferenceQualification,
    // Explicit pending-envelope guard exercise; absent from product builds.
    #[cfg(all(test, feature = "db-tests"))]
    PendingEnvelopeProduction,
}

impl TaskScalarUse {
    fn profile(self) -> Result<&'static TaskScalarProfile, sqlx::Error> {
        match self {
            Self::Production => TaskScalarProfile::compiled(),
            #[cfg(all(test, feature = "db-tests"))]
            Self::ReferenceQualification | Self::PendingEnvelopeProduction => {
                TaskScalarProfile::pending_envelope()
            }
        }
    }

    fn require_runtime(self, profile: &TaskScalarProfile) -> Result<(), sqlx::Error> {
        match self {
            Self::Production => profile.require_runtime(),
            #[cfg(all(test, feature = "db-tests"))]
            Self::ReferenceQualification => Ok(()),
            #[cfg(all(test, feature = "db-tests"))]
            Self::PendingEnvelopeProduction => profile.require_runtime(),
        }
    }

    fn locale(self, profile: &TaskScalarProfile) -> Result<TaskTextLocale, sqlx::Error> {
        match self {
            Self::Production => TaskTextLocale::for_profile(profile),
            #[cfg(all(test, feature = "db-tests"))]
            Self::ReferenceQualification => TaskTextLocale::new(),
            #[cfg(all(test, feature = "db-tests"))]
            Self::PendingEnvelopeProduction => TaskTextLocale::for_profile(profile),
        }
    }
}

async fn list_project_tasks_backend_with_use(
    backend: &Backend,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
    query: &ParsedTaskListQuery,
    scalar_use: TaskScalarUse,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_project_tasks(pool, workspace, project, actor, credential, query).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.session_is_live(actor, credential).await? {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace).await?
            || !op
                .project_permission_by_id(workspace, actor, project)
                .await?
                .is_some_and(|permission| permission.at_least(ProjectPermission::View))
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!("PostgreSQL uses the preserved project reader")
        };
        family.require_tenant(workspace)?;
        list_project_tasks_family(family, workspace, project, actor, query, scalar_use).await
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Ok(Err(refusal)) => Some(Box::new(ProjectTasksReadRefusal(refusal))),
            Err(error) => Some(Box::new(error)),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}

#[derive(Debug, thiserror::Error)]
#[error("project tasks read refused: {0:?}")]
struct ProjectTasksReadRefusal(ProjectDbError);

/// Exact typed values; decimal text is never cast to REAL or an integer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TaskScalar {
    Text(String),
    Number(bigdecimal::BigDecimal),
    Date(NaiveDate),
    Instant(DateTime<Utc>),
    Boolean(bool),
    Integer(i32),
}

impl TaskScalar {
    fn from_policy(policy: &ScalarPolicy) -> Result<Self, sqlx::Error> {
        Ok(match policy {
            ScalarPolicy::Text(value) => Self::Text(value.clone()),
            ScalarPolicy::Number(value) => Self::Number(decimal_value(value)?),
            ScalarPolicy::Date(value) => Self::Date(*value),
            ScalarPolicy::Instant(value) => Self::Instant(task_query_instant(*value)?),
            ScalarPolicy::Boolean(value) => Self::Boolean(*value),
        })
    }
}
// PostgreSQL REL_18_3 ParseFractionalSecond parses the fractional decimal
// with strtod and uses rint(frac * 1000000). Delegate decimal parsing and
// ties-to-even rounding to the standard library; chrono owns calendar/carry.
// This is a query adapter, not a lossy replacement of the storage codec.
fn task_query_instant(at: DateTime<Utc>) -> Result<DateTime<Utc>, sqlx::Error> {
    let nanos = at.timestamp_subsec_nanos();
    // chrono represents a parsed leap second with nanos >= 1_000_000_000.
    let whole = i64::from(nanos / 1_000_000_000);
    let fraction = format!("0.{:09}", nanos % 1_000_000_000)
        .parse::<f64>()
        .map_err(|_| sqlx::Error::Protocol("invalid Task query fraction".into()))?;
    let micros = (fraction * 1_000_000.0).round_ties_even() as u32;
    let second = at
        .timestamp()
        .checked_add(whole)
        .and_then(|second| second.checked_add(i64::from(micros / 1_000_000)))
        .ok_or_else(|| sqlx::Error::Protocol("Task query instant overflow".into()))?;
    DateTime::from_timestamp(second, (micros % 1_000_000) * 1000)
        .ok_or_else(|| sqlx::Error::Protocol("Task query instant out of range".into()))
}

fn decimal_value(value: &str) -> Result<bigdecimal::BigDecimal, sqlx::Error> {
    bigdecimal::BigDecimal::from_str(value)
        .map_err(|_| sqlx::Error::Protocol("invalid stored Task decimal".into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskOrderCell {
    value: Option<TaskScalar>,
    desc: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskPageKey {
    cells: Vec<TaskOrderCell>,
    id: Uuid,
}

fn task_scalar_error(message: &'static str) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

fn task_scalar_sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct TaskScalarReference {
    encoding: String,
    provider: String,
    deterministic: bool,
    lc_collate: String,
    lc_ctype: String,
    collation_version: String,
    tzdata_version: String,
    image_digest: String,
    names_sha256: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskScalarRuntime {
    arch: String,
    glibc_version: String,
    locale_archive_sha256: String,
    qualification_sha256: String,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct TaskScalarZone {
    name: String,
    tzif_base64: String,
    sha256: String,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskScalarProfile {
    schema_version: u32,
    reference: TaskScalarReference,
    runtime_profiles: Vec<TaskScalarRuntime>,
    zones: Vec<TaskScalarZone>,
    #[serde(skip)]
    identity: String,
    #[serde(skip)]
    data_identity: String,
    #[serde(skip)]
    decoded_zones: Vec<Vec<u8>>,
    #[serde(skip)]
    runtime_checked: std::sync::OnceLock<Result<(), &'static str>>,
}

// Immutable PG18 reference plus the exact canonical x86_64 runtime receipt.
// Admission remains bound to the exact GNU version and locale archive; other
// archives and architectures require separate qualification, never a fallback.
const TASK_SCALAR_PROFILE_SHA256: &str =
    "a5922390ade58f265bcb06e30450ad6d431d8bc4112d9626e2715ad818977ae3";

impl TaskScalarProfile {
    fn decode_sealed(raw: &str, expected: &str) -> Result<Self, &'static str> {
        if task_scalar_sha256(raw.as_bytes()) != expected {
            return Err("Task scalar compiled profile checksum mismatch");
        }
        Self::decode(raw)
    }

    fn decode(raw: &str) -> Result<Self, &'static str> {
        use base64::Engine;
        let mut profile: Self =
            serde_json::from_str(raw).map_err(|_| "Task scalar profile schema invalid")?;
        let r = &profile.reference;
        if profile.schema_version != 1
            || r.encoding != "UTF8"
            || r.provider != "c"
            || !r.deterministic
            || r.lc_collate != "en_US.utf8"
            || r.lc_ctype != "en_US.utf8"
            || r.collation_version != "2.41"
            || r.tzdata_version != "2026a"
            || r.image_digest
                != "sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
            || r.names_sha256 != "104662bf43ab373bc83f5999f5a18bc5ac094baeeca996e1a8f761fad8279c8c"
        {
            return Err("Task scalar reference profile mismatch");
        }
        if profile.zones.len() != 487
            || profile
                .zones
                .windows(2)
                .any(|pair| pair[0].name >= pair[1].name)
            || task_scalar_sha256(
                profile
                    .zones
                    .iter()
                    .map(|zone| zone.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_bytes(),
            ) != r.names_sha256
        {
            return Err("Task scalar zone admission mismatch");
        }
        for zone in &profile.zones {
            let data = base64::engine::general_purpose::STANDARD
                .decode(&zone.tzif_base64)
                .map_err(|_| "Task scalar TZif base64 invalid")?;
            if task_scalar_sha256(&data) != zone.sha256 {
                return Err("Task scalar TZif checksum mismatch");
            }
            tz::TimeZone::from_tz_data(&data).map_err(|_| "Task scalar TZif invalid")?;
            profile.decoded_zones.push(data);
        }
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && value != "0".repeat(64)
        };
        let mut architectures = HashSet::new();
        for runtime in &profile.runtime_profiles {
            if !matches!(runtime.arch.as_str(), "x86_64" | "aarch64")
                || !architectures.insert(runtime.arch.clone())
                || runtime.glibc_version.is_empty()
                || !runtime
                    .glibc_version
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.')
                || !hash(&runtime.locale_archive_sha256)
                || !hash(&runtime.qualification_sha256)
            {
                return Err("Task scalar runtime profile invalid");
            }
        }
        // Qualification binds immutable reference+zone input, independently
        // of the envelope that later carries the qualification receipt itself.
        let data =
            serde_json::to_vec(&(profile.schema_version, &profile.reference, &profile.zones))
                .map_err(|_| "Task scalar data identity invalid")?;
        profile.data_identity = task_scalar_sha256(&data);
        profile.identity = task_scalar_sha256(raw.as_bytes());
        Ok(profile)
    }

    fn compiled() -> Result<&'static Self, sqlx::Error> {
        static PROFILE: std::sync::OnceLock<Result<TaskScalarProfile, &'static str>> =
            std::sync::OnceLock::new();
        PROFILE
            .get_or_init(|| {
                let raw = include_str!("task_scalar_pg18_profile.json");
                Self::decode_sealed(raw, TASK_SCALAR_PROFILE_SHA256)
            })
            .as_ref()
            .map_err(|error| task_scalar_error(error))
    }

    // Derive a pending fixture from the sealed real input, changing only the
    // admission envelope. This cannot be selected by a product endpoint.
    #[cfg(all(test, feature = "db-tests"))]
    fn pending_envelope() -> Result<&'static Self, sqlx::Error> {
        static PROFILE: std::sync::OnceLock<Result<TaskScalarProfile, &'static str>> =
            std::sync::OnceLock::new();
        PROFILE
            .get_or_init(|| {
                let raw = include_str!("task_scalar_pg18_profile.json");
                Self::decode_sealed(raw, TASK_SCALAR_PROFILE_SHA256)?;
                let mut value: serde_json::Value =
                    serde_json::from_str(raw).map_err(|_| "Task scalar profile schema invalid")?;
                value["runtime_profiles"] = serde_json::json!([]);
                Self::decode(&value.to_string())
            })
            .as_ref()
            .map_err(|error| task_scalar_error(error))
    }

    fn require_runtime(&self) -> Result<(), sqlx::Error> {
        // An environment-provided search path must not replace qualified data,
        // even after immutable package validation has been cached.
        if std::env::var_os("LOCPATH").is_some() {
            return Err(task_scalar_error("Task GNU locale search path unsupported"));
        }
        self.runtime_checked
            .get_or_init(|| {
                #[cfg(all(target_os = "linux", target_env = "gnu"))]
                {
                    let runtime = self
                        .runtime_profiles
                        .iter()
                        .find(|runtime| runtime.arch == std::env::consts::ARCH)
                        .ok_or("Task scalar runtime qualification unavailable")?;
                    // SAFETY: GNU returns a static, NUL-terminated version string.
                    let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }
                        .to_str()
                        .map_err(|_| "Task GNU version invalid")?;
                    if version != runtime.glibc_version {
                        return Err("Task GNU version mismatch");
                    }
                    let archive = std::fs::read("/usr/lib/locale/locale-archive")
                        .map_err(|_| "Task GNU locale archive unavailable")?;
                    if task_scalar_sha256(&archive) != runtime.locale_archive_sha256 {
                        return Err("Task GNU locale archive mismatch");
                    }
                    Ok(())
                }
                #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
                Err("Task scalar GNU target unavailable")
            })
            .as_ref()
            .copied()
            .map_err(|error| task_scalar_error(error))
    }
}

/// This guard exists only during synchronous batch processing, never across
/// a DB await. GNU owns comparison/case mapping; no process locale is changed.
struct TaskTextLocale {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    handle: libc::locale_t,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    // GNU's x86_64/aarch64 ABI uses unsigned int for wint_t; target ABI and
    // implementation/data provenance require the ROOT qualification receipt.
    fn strcoll_l(
        left: *const libc::c_char,
        right: *const libc::c_char,
        locale: libc::locale_t,
    ) -> libc::c_int;
    fn towlower_l(value: libc::c_uint, locale: libc::locale_t) -> libc::c_uint;
}

impl TaskTextLocale {
    fn for_profile(profile: &TaskScalarProfile) -> Result<Self, sqlx::Error> {
        profile.require_runtime()?;
        Self::new()
    }

    // Qualification exercises the actual bridge on PREP data; production must
    // use for_profile, requiring a separately sealed architecture receipt.
    fn new() -> Result<Self, sqlx::Error> {
        Self::open(c"en_US.utf8")
    }

    fn open(name: &std::ffi::CStr) -> Result<Self, sqlx::Error> {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            // SAFETY: valid CStr, valid masks and no borrowed base; production
            // fixes the locale name. The successful handle is exclusively owned.
            let handle = unsafe {
                libc::newlocale(
                    libc::LC_COLLATE_MASK | libc::LC_CTYPE_MASK,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                )
            };
            if handle.is_null() {
                return Err(task_scalar_error("Task GNU locale unavailable"));
            }
            Ok(Self { handle })
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        {
            let _ = name;
            Err(task_scalar_error("Task scalar GNU target unavailable"))
        }
    }

    fn lower_literal(&self, value: &str) -> Result<String, sqlx::Error> {
        if value.contains('\0') {
            return Err(task_scalar_error("Task scalar text contains NUL"));
        }
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            value
                .chars()
                .map(|value| {
                    // SAFETY: valid Unicode scalar fits GNU wint_t; the owned,
                    // unmodified locale remains live throughout this call.
                    char::from_u32(unsafe { towlower_l(u32::from(value), self.handle) })
                        .ok_or_else(|| task_scalar_error("Task GNU case mapping invalid"))
                })
                .collect()
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        Err(task_scalar_error("Task scalar GNU target unavailable"))
    }

    fn compare(&self, left: &str, right: &str) -> Result<Ordering, sqlx::Error> {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            let a = std::ffi::CString::new(left)
                .map_err(|_| task_scalar_error("Task scalar text contains NUL"))?;
            let b = std::ffi::CString::new(right)
                .map_err(|_| task_scalar_error("Task scalar text contains NUL"))?;
            // SAFETY: both C strings and the exclusively owned locale outlive
            // the call. errno is thread-local; no await/thread hop occurs here.
            let (order, error) = unsafe {
                *libc::__errno_location() = 0;
                let order = strcoll_l(a.as_ptr(), b.as_ptr(), self.handle);
                (order, *libc::__errno_location())
            };
            if error != 0 {
                return Err(task_scalar_error("Task GNU comparison failed"));
            }
            // PostgreSQL's deterministic collation resolves locale-equal
            // strings with byte order; equality never normalizes code points.
            Ok(order
                .cmp(&0)
                .then_with(|| left.as_bytes().cmp(right.as_bytes())))
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        {
            let _ = (left, right);
            Err(task_scalar_error("Task scalar GNU target unavailable"))
        }
    }
}

impl Drop for TaskTextLocale {
    fn drop(&mut self) {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        // SAFETY: newlocale succeeded, ownership was never shared/transferred,
        // and every comparison has completed before this synchronous drop.
        unsafe {
            libc::freelocale(self.handle)
        }
    }
}

fn compare_task_page_key(
    left: &TaskPageKey,
    right: &TaskPageKey,
    sort: &[ViewSort],
    locale: &TaskTextLocale,
) -> Result<Ordering, sqlx::Error> {
    if left.cells.len() != sort.len() || right.cells.len() != sort.len() {
        return Err(task_scalar_error("Task scalar sort plan mismatch"));
    }
    for ((left, right), term) in left.cells.iter().zip(&right.cells).zip(sort) {
        let desc = term.direction == SortDirection::Desc;
        if left.desc != desc || right.desc != desc {
            return Err(task_scalar_error("Task scalar sort direction mismatch"));
        }
        let order = match (&left.value, &right.value) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => {
                if std::mem::discriminant(left) != std::mem::discriminant(right) {
                    return Err(task_scalar_error("Task scalar sort type mismatch"));
                }
                if [left, right]
                    .iter()
                    .any(|value| matches!(value, TaskScalar::Text(text) if text.contains('\0')))
                {
                    return Err(task_scalar_error("Task scalar text contains NUL"));
                }
                let order = match (left, right) {
                    (TaskScalar::Text(a), TaskScalar::Text(b))
                        if !matches!(term.field, SortField::Title | SortField::Rank) =>
                    {
                        locale.compare(a, b)?
                    }
                    _ => left.cmp(right),
                };
                if desc {
                    order.reverse()
                } else {
                    order
                }
            }
        };
        if order != Ordering::Equal {
            return Ok(order);
        }
    }
    Ok(left.id.cmp(&right.id))
}

#[derive(Debug, Clone)]
struct TaskProjection {
    id: Uuid,
    number: i32,
    title: String,
    priority: String,
    status: Uuid,
    start: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
    rank: String,
    created: DateTime<Utc>,
    updated: DateTime<Utc>,
    status_rank: String,
    scalars: HashMap<Uuid, TaskScalar>,
}

impl TaskProjection {
    fn from_row(row: &FamilyRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.cell(0)?.id()?,
            number: row.cell(1)?.int32()?,
            title: row.cell(2)?.string()?,
            priority: row.cell(3)?.string()?,
            status: row.cell(4)?.id()?,
            start: row.cell(5)?.optional(Cell::date)?,
            due_date: row.cell(6)?.optional(Cell::date)?,
            due_at: row.cell(7)?.optional(Cell::datetime)?,
            rank: row.cell(8)?.string()?,
            created: row.cell(9)?.datetime()?,
            updated: row.cell(10)?.datetime()?,
            status_rank: row.cell(11)?.string()?,
            scalars: HashMap::new(),
        })
    }

    fn due(&self, zone: &TaskTimeZone) -> Result<Option<NaiveDate>, sqlx::Error> {
        match (self.due_date, self.due_at) {
            (Some(date), _) => Ok(Some(date)),
            (None, Some(at)) => zone.date(at).map(Some),
            (None, None) => Ok(None),
        }
    }

    fn page_key(&self, sort: &[ViewSort], zone: &TaskTimeZone) -> Result<TaskPageKey, sqlx::Error> {
        let due = self.due(zone)?;
        let cells = sort
            .iter()
            .map(|entry| TaskOrderCell {
                desc: entry.direction == SortDirection::Desc,
                value: match entry.field {
                    SortField::Field(id) => self.scalars.get(&id).cloned(),
                    SortField::Due => due.map(TaskScalar::Date),
                    SortField::Created => Some(TaskScalar::Instant(self.created)),
                    SortField::Updated => Some(TaskScalar::Instant(self.updated)),
                    SortField::Number => Some(TaskScalar::Integer(self.number)),
                    SortField::Priority => Some(TaskScalar::Integer(
                        crate::tasks::list_query::priority_rank(&self.priority),
                    )),
                    SortField::Title => Some(TaskScalar::Text(self.title.clone())),
                    SortField::Rank => Some(TaskScalar::Text(self.rank.clone())),
                    SortField::Status => Some(TaskScalar::Text(self.status_rank.clone())),
                },
            })
            .collect::<Vec<_>>();
        if cells.iter().any(
            |cell| matches!(&cell.value, Some(TaskScalar::Text(value)) if value.contains('\0')),
        ) {
            return Err(task_scalar_error("Task scalar text contains NUL"));
        }
        Ok(TaskPageKey { cells, id: self.id })
    }

    fn cursor_key(&self, sort: &[ViewSort], zone: &TaskTimeZone) -> Result<String, sqlx::Error> {
        // Family cursors have their own version/catalog/TZDB fingerprint; the
        // original PostgreSQL key and token spelling remain unchanged.
        let tokens: Vec<_> = sort
            .iter()
            .filter_map(|entry| match entry.field {
                SortField::Field(id) => Some((
                    id,
                    self.scalars.get(&id).map(|value| match value {
                        TaskScalar::Number(number) => number.normalized().to_string(),
                        TaskScalar::Text(text) => text.clone(),
                        TaskScalar::Date(date) => date.to_string(),
                        TaskScalar::Instant(at) => at.to_rfc3339(),
                        TaskScalar::Boolean(flag) => flag.to_string(),
                        TaskScalar::Integer(number) => number.to_string(),
                    }),
                )),
                _ => None,
            })
            .collect();
        Ok(cursor_key_for_row(
            sort,
            self.id,
            self.created,
            self.updated,
            self.number,
            &self.title,
            &self.rank,
            &self.priority,
            &self.status_rank,
            self.due(zone)?,
            &tokens,
        ))
    }
}

struct TaskTimeZone {
    name: String,
    zone: tz::TimeZone,
}
impl TaskTimeZone {
    fn from_name(name: &str) -> Result<Self, sqlx::Error> {
        let profile = TaskScalarProfile::compiled()?;
        // Exact PG catalog admission, including its real configured aliases;
        // only unknown/case-mismatched names retain the original UTC fallback.
        let index = profile
            .zones
            .binary_search_by(|zone| zone.name.as_str().cmp(name))
            .or_else(|_| {
                profile
                    .zones
                    .binary_search_by(|zone| zone.name.as_str().cmp("UTC"))
            })
            .map_err(|_| task_scalar_error("Task scalar UTC data unavailable"))?;
        let zone = tz::TimeZone::from_tz_data(&profile.decoded_zones[index])
            .map_err(|_| task_scalar_error("Task scalar TZif invalid"))?;
        Ok(Self {
            name: profile.zones[index].name.clone(),
            zone,
        })
    }
    fn date(&self, at: DateTime<Utc>) -> Result<NaiveDate, sqlx::Error> {
        let local = tz::DateTime::from_timespec(
            at.timestamp(),
            at.timestamp_subsec_nanos(),
            self.zone.as_ref(),
        )
        .map_err(|_| sqlx::Error::Protocol("Task time zone projection failed".into()))?;
        NaiveDate::from_ymd_opt(
            local.year(),
            u32::from(local.month()),
            u32::from(local.month_day()),
        )
        .ok_or_else(|| sqlx::Error::Protocol("Task projected date out of range".into()))
    }
}

#[derive(Debug, Eq, PartialEq)]
struct TaskPageEntry {
    key: TaskPageKey,
    cursor_key: String,
}
fn retain_task_top_k(
    selected: &mut Vec<TaskPageEntry>,
    entry: TaskPageEntry,
    capacity: usize,
    sort: &[ViewSort],
    locale: &TaskTextLocale,
) -> Result<(), sqlx::Error> {
    // Validate even the first entry before mutating the bounded selection.
    compare_task_page_key(&entry.key, &entry.key, sort, locale)?;
    let (mut low, mut high) = (0, selected.len());
    while low < high {
        let middle = low + (high - low) / 2;
        if compare_task_page_key(&selected[middle].key, &entry.key, sort, locale)? == Ordering::Less
        {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low < capacity {
        selected.insert(low, entry);
        selected.truncate(capacity);
    }
    Ok(())
}

/// Both local and remote family query APIs collect one statement's rows. This
/// fixed projection (not the body/recurrence DTO) is therefore bounded to 32;
/// EOF, not an arbitrary candidate ceiling, proves every count and page.
async fn task_projection_batch(
    tx: &mut FamilyTx,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    query: &ParsedTaskListQuery,
    after: Option<Uuid>,
    anchor: Option<Uuid>,
) -> Result<Vec<TaskProjection>, sqlx::Error> {
    let filters = &query.view.filters;
    let optional_id = |id: Option<Uuid>| id.map(Cell::uuid).unwrap_or(Cell::Null);
    let assignee = filters.assignee_id.as_ref().map(|assignee| match assignee {
        AssigneeFilter::Me => actor,
        AssigneeFilter::User(id) => *id,
    });
    let args = [
        Cell::uuid(workspace),
        Cell::uuid(project),
        optional_id(after),
        Cell::Integer(i64::from(query.archived)),
        filters
            .task_type
            .as_ref()
            .map(|value| Cell::Text(value.clone()))
            .unwrap_or(Cell::Null),
        optional_id(filters.status_id),
        filters
            .priority
            .as_ref()
            .map(|value| Cell::Text(value.clone()))
            .unwrap_or(Cell::Null),
        Cell::Integer(i64::from(filters.open_only)),
        optional_id(filters.label_id),
        optional_id(filters.milestone_id),
        optional_id(assignee),
        optional_id(anchor),
    ];
    tx.query(
        "SELECT t.id,t.number,t.title,t.priority,t.status_id,t.start_date,t.due_date,t.due_at,t.sort_key,t.created_at,t.updated_at,s.sort_key FROM tasks t JOIN statuses s ON s.workspace_id=t.workspace_id AND s.project_id=t.project_id AND s.id=t.status_id WHERE t.workspace_id=?1 AND t.project_id=?2 AND t.deleted_at IS NULL AND ((?12 IS NOT NULL AND t.id=?12) OR (?12 IS NULL AND (?3 IS NULL OR t.id>?3) AND (t.archived_at IS NOT NULL)=?4 AND (?5 IS NULL OR t.type=?5) AND (?6 IS NULL OR t.status_id=?6) AND (?7 IS NULL OR t.priority=?7) AND (?8=0 OR s.category NOT IN ('done','canceled')) AND (?9 IS NULL OR EXISTS(SELECT 1 FROM task_labels l WHERE l.workspace_id=t.workspace_id AND l.task_id=t.id AND l.label_id=?9)) AND (?10 IS NULL OR t.milestone_id=?10) AND (?11 IS NULL OR EXISTS(SELECT 1 FROM task_assignees a WHERE a.workspace_id=t.workspace_id AND a.task_id=t.id AND a.user_id=?11)))) ORDER BY t.id LIMIT 32",
        &args,
    ).await?.iter().map(TaskProjection::from_row).collect()
}

async fn task_projection_values(
    tx: &mut FamilyTx,
    workspace: Uuid,
    plan: &SelectedTaskView,
    candidates: &mut [TaskProjection],
) -> Result<HashSet<Uuid>, sqlx::Error> {
    let ids: Vec<_> = candidates.iter().map(|row| row.id).collect();
    let scalar_ids: Vec<_> = plan
        .catalog
        .iter()
        .filter(|field| value_column(&field.field_type).is_some())
        .map(|field| field.id)
        .collect();
    let mut values = HashMap::new();
    if !scalar_ids.is_empty() && !ids.is_empty() {
        let rows = tx.query(
            "SELECT ci.task_id,v.field_id,v.value_text,v.value_number,v.value_date,v.value_ts,v.value_bool FROM collection_items ci JOIN collection_values v ON v.workspace_id=ci.workspace_id AND v.collection_id=ci.collection_id AND v.item_id=ci.id WHERE ci.workspace_id=?1 AND hex(ci.task_id) IN (SELECT value FROM json_each(?2)) AND hex(v.field_id) IN (SELECT value FROM json_each(?3))",
            &[Cell::uuid(workspace), selected_ids(&ids), selected_ids(&scalar_ids)],
        ).await?;
        for row in rows {
            let task = row.cell(0)?.id()?;
            let field = row.cell(1)?.id()?;
            let kind = &plan
                .catalog
                .iter()
                .find(|entry| entry.id == field)
                .ok_or_else(|| sqlx::Error::Protocol("Task scalar catalog mismatch".into()))?
                .field_type;
            let value = match kind.as_str() {
                "text" | "paragraph" => TaskScalar::Text(row.cell(2)?.string()?),
                "number" => TaskScalar::Number(decimal_value(&row.cell(3)?.string()?)?),
                "date" => TaskScalar::Date(row.cell(4)?.date()?),
                "datetime" => TaskScalar::Instant(row.cell(5)?.datetime()?),
                "checkbox" => TaskScalar::Boolean(row.cell(6)?.boolean()?),
                _ => return Err(sqlx::Error::Protocol("invalid Task scalar type".into())),
            };
            if values.insert((task, field), value).is_some() {
                return Err(sqlx::Error::Protocol(
                    "duplicate Task scalar projection".into(),
                ));
            }
        }
    }
    let mut matches: HashSet<_> = ids.iter().copied().collect();
    for (field, policy) in &plan.predicates {
        match policy {
            CustomPolicy::Scalar { equals, .. } => {
                let expected = equals.as_ref().map(TaskScalar::from_policy).transpose()?;
                matches.retain(|task| values.get(&(*task, *field)) == expected.as_ref());
            }
            CustomPolicy::Set { people, equals } => {
                // EXISTS makes one result per candidate even for an unbounded
                // historical set. It does not copy every option/person row.
                let sql = if *people {
                    "SELECT t.id,EXISTS(SELECT 1 FROM collection_items ci JOIN collection_people v ON v.workspace_id=ci.workspace_id AND v.collection_id=ci.collection_id AND v.item_id=ci.id WHERE ci.workspace_id=t.workspace_id AND ci.task_id=t.id AND v.field_id=?3 AND (?4 IS NULL OR v.user_id=?4)) FROM tasks t WHERE t.workspace_id=?1 AND hex(t.id) IN (SELECT value FROM json_each(?2))"
                } else {
                    "SELECT t.id,EXISTS(SELECT 1 FROM collection_items ci JOIN collection_choices v ON v.workspace_id=ci.workspace_id AND v.collection_id=ci.collection_id AND v.item_id=ci.id WHERE ci.workspace_id=t.workspace_id AND ci.task_id=t.id AND v.field_id=?3 AND (?4 IS NULL OR v.option_id=?4)) FROM tasks t WHERE t.workspace_id=?1 AND hex(t.id) IN (SELECT value FROM json_each(?2))"
                };
                let rows = tx
                    .query(
                        sql,
                        &[
                            Cell::uuid(workspace),
                            selected_ids(&ids),
                            Cell::uuid(*field),
                            equals.map(Cell::uuid).unwrap_or(Cell::Null),
                        ],
                    )
                    .await?;
                for row in rows {
                    if row.cell(1)?.boolean()? != equals.is_some() {
                        matches.remove(&row.cell(0)?.id()?);
                    }
                }
            }
        }
    }
    for candidate in candidates {
        for field in &scalar_ids {
            if let Some(value) = values.remove(&(candidate.id, *field)) {
                candidate.scalars.insert(*field, value);
            }
        }
    }
    Ok(matches)
}

fn task_projection_matches(
    row: &TaskProjection,
    query: &ParsedTaskListQuery,
    zone: &TaskTimeZone,
    locale: &TaskTextLocale,
) -> Result<bool, sqlx::Error> {
    if row.created > task_query_instant(query.as_of)? {
        return Ok(false);
    }
    if let Some(title) = &query.view.filters.title {
        // '%'/'_'/'\\' remain literal substring characters, not LIKE syntax.
        // GNU maps individual code points, as PG's UTF8 libc lower/ILIKE
        // paths do; Rust's full/context-sensitive lower is a different policy.
        if !locale
            .lower_literal(&row.title)?
            .contains(&locale.lower_literal(title)?)
        {
            return Ok(false);
        }
    }
    if let Some(before) = query.view.filters.due_before {
        if !row.due(zone)?.is_some_and(|due| due <= before) {
            return Ok(false);
        }
    }
    if let (Some(from), Some(to)) = (query.from, query.to) {
        // The original window uses UTC endpoints; actor-local Due is a
        // separate sort/filter expression. PostgreSQL LEAST/GREATEST ignore
        // a single NULL, and a wholly undated row does not intersect a window.
        let due = row
            .due_date
            .or_else(|| row.due_at.map(|at| at.date_naive()));
        let ends = row.start.into_iter().chain(due).collect::<Vec<_>>();
        if !ends.iter().min().is_some_and(|date| *date <= to)
            || !ends.iter().max().is_some_and(|date| *date >= from)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn list_project_tasks_family(
    tx: &mut FamilyTx,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    query: &ParsedTaskListQuery,
    scalar_use: TaskScalarUse,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    let ws_project = [Cell::uuid(workspace), Cell::uuid(project)];
    for (sql, value) in [
        ("SELECT EXISTS(SELECT 1 FROM labels WHERE workspace_id=?1 AND project_id=?2 AND id=?3)", query.view.filters.label_id),
        ("SELECT EXISTS(SELECT 1 FROM milestones WHERE workspace_id=?1 AND project_id=?2 AND id=?3)", query.view.filters.milestone_id),
    ] {
        if let Some(id) = value {
            if !tx.query(sql, &[ws_project[0].clone(),ws_project[1].clone(),Cell::uuid(id)]).await?
                .first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.boolean()? {
                return Ok(Err(ProjectDbError::InvalidInput));
            }
        }
    }
    if let Some(AssigneeFilter::User(user)) = query.view.filters.assignee_id {
        if !tx.query("SELECT EXISTS(SELECT 1 FROM memberships m JOIN users u ON u.id=m.user_id WHERE m.workspace_id=?1 AND u.id=?2 AND u.deleted_at IS NULL)", &[Cell::uuid(workspace),Cell::uuid(user)]).await?
            .first().ok_or(sqlx::Error::RowNotFound)?.cell(0)?.boolean()? {
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    let name_rows = tx
        .query(
            "SELECT timezone FROM users WHERE id=?1",
            &[Cell::uuid(actor)],
        )
        .await?;
    let zone = TaskTimeZone::from_name(
        &name_rows
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .string()?,
    )?;
    let profile = scalar_use.profile()?;
    scalar_use.require_runtime(profile)?;
    let plan = match prepare_selected_task_view(
        tx,
        ViewScope {
            workspace_id: workspace,
            project_id: Some(project),
            collection_id: None,
            kind: RootKind::Task,
        },
        &query.view,
    )
    .await?
    {
        Ok(plan) => plan,
        Err(_) => return Ok(Err(ProjectDbError::InvalidInput)),
    };
    let fingerprint = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(json!({
        "policy": "selected-task-2-gnu", "scalar_profile": &profile.identity,
        "scalar_data": &profile.data_identity,
        "scalar_arch": std::env::consts::ARCH,
        "query": filter_fingerprint(workspace, Some(project), &zone.name, query),
        "actor": actor,
        "catalog": plan.catalog.iter().map(|field| (field.id, field.version)).collect::<Vec<_>>()
        }).to_string().as_bytes()))
    };
    if query
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.f != fingerprint)
    {
        return Ok(Err(ProjectDbError::InvalidCursor));
    }
    let anchor = if let Some(cursor) = &query.cursor {
        let mut rows =
            task_projection_batch(tx, workspace, project, actor, query, None, Some(cursor.id))
                .await?;
        task_projection_values(tx, workspace, &plan, &mut rows).await?;
        let Some(row) = rows.first() else {
            return Ok(Err(ProjectDbError::InvalidCursor));
        };
        if row.cursor_key(&plan.sort, &zone)? != cursor.key {
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
        Some(row.page_key(&plan.sort, &zone)?)
    } else {
        None
    };
    let capacity = usize::try_from(query.limit)
        .map_err(|_| sqlx::Error::Protocol("invalid parsed Task limit".into()))?
        .checked_add(1)
        .ok_or_else(|| sqlx::Error::Protocol("Task page limit overflow".into()))?;
    let mut counts = BTreeMap::<Uuid, i64>::new();
    let mut selected = Vec::with_capacity(capacity);
    let mut after = None;
    loop {
        let mut rows =
            task_projection_batch(tx, workspace, project, actor, query, after, None).await?;
        if rows.is_empty() {
            break;
        }
        let matches = task_projection_values(tx, workspace, &plan, &mut rows).await?;
        {
            // A raw GNU locale handle never enters the async suspension state.
            // The next batch's query runs only after this guard has been dropped.
            let locale = scalar_use.locale(profile)?;
            for row in rows {
                if after.is_some_and(|previous| row.id <= previous) {
                    return Err(sqlx::Error::Protocol("Task scan did not advance".into()));
                }
                after = Some(row.id);
                if !matches.contains(&row.id)
                    || !task_projection_matches(&row, query, &zone, &locale)?
                {
                    continue;
                }
                let count = counts.entry(row.status).or_default();
                *count = count
                    .checked_add(1)
                    .ok_or_else(|| sqlx::Error::Protocol("Task count overflow".into()))?;
                let key = row.page_key(&plan.sort, &zone)?;
                if let Some(anchor) = &anchor {
                    if compare_task_page_key(&key, anchor, &plan.sort, &locale)?
                        != Ordering::Greater
                    {
                        continue;
                    }
                }
                retain_task_top_k(
                    &mut selected,
                    TaskPageEntry {
                        key,
                        cursor_key: row.cursor_key(&plan.sort, &zone)?,
                    },
                    capacity,
                    &plan.sort,
                    &locale,
                )?;
            }
            drop(locale);
        }
    }
    let has_more = selected.len() == capacity;
    selected.truncate(capacity - 1);
    let next_cursor = if has_more {
        selected.last().map(|last| {
            encode_cursor(&TaskListCursor {
                id: last.key.id,
                key: last.cursor_key.clone(),
                f: fingerprint,
                as_of: query.as_of,
            })
        })
    } else {
        None
    };
    let ids = selected
        .iter()
        .map(|entry| entry.key.id)
        .collect::<Vec<_>>();
    let mut items = hydrate_selected_tasks(tx, workspace, project, &ids).await?;
    let ordered = ids
        .into_iter()
        .map(|id| {
            items
                .remove(&id)
                .ok_or_else(|| sqlx::Error::Protocol("selected Task hydration missing".into()))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(Ok(TaskListPage {
        items: ordered,
        status_counts: counts.into_iter().collect(),
        next_cursor,
    }))
}

async fn hydrate_selected_tasks(
    tx: &mut FamilyTx,
    workspace: Uuid,
    project: Uuid,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, TaskListItemRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let args = [
        Cell::uuid(workspace),
        Cell::uuid(project),
        selected_ids(ids),
    ];
    let rows = tx.query("SELECT id,project_id,number,title,type,priority,status_id,start_date,due_date,due_at,estimate,parent_id,milestone_id,sort_key,schema_version,version,archived_at,created_by,created_at,updated_at,recurrence FROM tasks WHERE workspace_id=?1 AND project_id=?2 AND deleted_at IS NULL AND hex(id) IN (SELECT value FROM json_each(?3))", &args).await?;
    let mut items = HashMap::with_capacity(rows.len());
    for row in rows {
        let record = map_task_detail_family_row(&row)?;
        let id = record.id;
        if items
            .insert(
                id,
                TaskListItemRow {
                    meta: row_to_meta(workspace, record, row.cell(20)?.optional(Cell::value)?),
                    assignee_ids: Vec::new(),
                    label_ids: Vec::new(),
                },
            )
            .is_some()
        {
            return Err(sqlx::Error::Protocol("duplicate Task hydration".into()));
        }
    }
    for (sql, assignees) in [
        ("SELECT task_id,user_id FROM task_assignees WHERE workspace_id=?1 AND hex(task_id) IN (SELECT value FROM json_each(?2)) ORDER BY task_id,user_id", true),
        ("SELECT task_id,label_id FROM task_labels WHERE workspace_id=?1 AND hex(task_id) IN (SELECT value FROM json_each(?2)) ORDER BY task_id,label_id", false),
    ] {
        for row in tx.query(sql, &[Cell::uuid(workspace),selected_ids(ids)]).await? {
            let item = items.get_mut(&row.cell(0)?.id()?)
                .ok_or_else(|| sqlx::Error::Protocol("Task reference hydration mismatch".into()))?;
            if assignees { item.assignee_ids.push(row.cell(1)?.id()?); }
            else { item.label_ids.push(row.cell(1)?.id()?); }
        }
    }
    Ok(items)
}

/// Source `listTasks(projectId = null)`: live tasks of every live project the
/// actor can currently view, for any workspace member (guests included).
pub async fn list_workspace_tasks(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    query: &ParsedTaskListQuery,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    list_tasks_in_scope(pool, workspace_id, None, actor_user_id, session_id, query).await
}

/// `$2` is the project id for a project list and the actor id for the
/// workspace-wide list, whose rows are limited to projects the actor can view.
async fn list_tasks_in_scope(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    actor_user_id: Uuid,
    session_id: Uuid,
    query: &ParsedTaskListQuery,
) -> Result<Result<TaskListPage, ProjectDbError>, sqlx::Error> {
    let time_zone = crate::db::dashboard::user_time_zone(pool, actor_user_id).await?;
    let fingerprint = filter_fingerprint(workspace_id, project_id, &time_zone, query);
    if let Some(cursor) = &query.cursor {
        if cursor.f != fingerprint {
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
    }
    let mut tx = begin_read(pool).await?;
    // Bounds the correlated filter/sort subqueries of a user-built view query.
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let scope_condition = match project_id {
        Some(project_id) => {
            let project = load_live_project(&mut tx, workspace_id, project_id).await?;
            let Some(project) = project else {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::NotFound));
            };
            if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
                .await?
                .at_least(ProjectPermission::View)
            {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::NotFound));
            }
            "t.project_id = $2".to_string()
        }
        None => {
            let Some(role) =
                crate::db::workspace::membership_role(&mut tx, workspace_id, actor_user_id).await?
            else {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::NotFound));
            };
            let visible = visible_project_sql_for_guest(
                "p",
                role == crate::db::workspace::WorkspaceRole::Guest,
                2,
            );
            format!(
                "EXISTS (
                    SELECT 1 FROM fvoci.projects p
                    WHERE p.workspace_id = t.workspace_id
                      AND p.id = t.project_id
                      AND p.deleted_at IS NULL
                      AND {visible}
                )"
            )
        }
    };
    let scope_bind = project_id.unwrap_or(actor_user_id);
    if let Some(label_id) = query.view.filters.label_id {
        let exists = match project_id {
            Some(project_id) => {
                project_label_exists(&mut tx, workspace_id, project_id, label_id).await?
            }
            None => workspace_row_exists(&mut tx, "labels", workspace_id, label_id).await?,
        };
        if !exists {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(milestone_id) = query.view.filters.milestone_id {
        let exists = match project_id {
            Some(project_id) => {
                project_milestone_exists(&mut tx, workspace_id, project_id, milestone_id).await?
            }
            None => workspace_row_exists(&mut tx, "milestones", workspace_id, milestone_id).await?,
        };
        if !exists {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(AssigneeFilter::User(user_id)) = query.view.filters.assignee_id {
        if !assignee_filter_member_exists(&mut tx, workspace_id, user_id).await? {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }

    let (mut base_conditions, mut base_binds) =
        task_list_filter_conditions(query, actor_user_id, scope_condition.clone());
    // Collection-backed parts of the view query (custom field filters,
    // dueBefore, field sorts) come from the shared compiler.
    let mut compiled_args = SqlArgs::starting_at(base_binds.len() + 3);
    let compiled = match compile_view_query(
        &mut tx,
        ViewScope {
            workspace_id,
            project_id,
            collection_id: None,
            kind: RootKind::Task,
        },
        &query.view,
        &CompileOptions {
            actor_user_id,
            time_zone: &time_zone,
            standard_filters: false,
        },
        "t",
        &mut compiled_args,
    )
    .await?
    {
        Ok(compiled) => compiled,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    };
    base_conditions.extend(compiled.conditions.iter().cloned());
    base_binds.extend(compiled_args.values.iter().cloned());
    let compiled_sorts = compiled_sort_terms(&compiled);
    let custom_fields: Vec<(Uuid, &'static str)> = compiled
        .catalog
        .iter()
        .filter(|field| compiled_sorts.contains_key(&SortField::Field(field.id)))
        .filter_map(|field| value_column(&field.field_type).map(|column| (field.id, column)))
        .collect();
    let mut conditions = base_conditions.clone();
    let mut binds = base_binds.clone();
    let sort = effective_sort_entries(&query.view.sort);
    if let Some(cursor) = &query.cursor {
        // The anchor row is subject to the same scope predicate as the page
        // (project, or visible projects for the workspace scope), so a cursor
        // naming a task the actor cannot see is rejected like a missing one.
        let anchor_sql = format!(
            r#"
            SELECT t.created_at, t.updated_at, t.id, t.number, t.title, t.sort_key, t.priority, t.status_id,
                   {due_sql} AS due,
                   (
                       SELECT st.sort_key
                       FROM fvoci.statuses st
                       WHERE st.workspace_id = t.workspace_id
                         AND st.project_id = t.project_id
                         AND st.id = t.status_id
                   ) AS status_sort_key
            FROM fvoci.tasks t
            WHERE t.workspace_id = $1
              AND {scope_condition}
              AND t.id = $3
              AND t.deleted_at IS NULL
            "#,
            due_sql = due_date_sql("t", "$4"),
        );
        let anchor: Option<TaskListCursorAnchor> = sqlx::query_as(&anchor_sql)
            .bind(workspace_id)
            .bind(scope_bind)
            .bind(cursor.id)
            .bind(&time_zone)
            .fetch_optional(&mut *tx)
            .await?;
        let Some((
            created_at,
            updated_at,
            id,
            number,
            title,
            sort_key,
            priority,
            _status_id,
            due,
            status_sort_key,
        )) = anchor
        else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        };
        let anchor_tokens =
            load_custom_sort_tokens(&mut tx, workspace_id, id, &custom_fields).await?;
        let key = cursor_key_for_row(
            &sort,
            id,
            created_at,
            updated_at,
            number,
            &title,
            &sort_key,
            &priority,
            &status_sort_key,
            due,
            &anchor_tokens,
        );
        if key != cursor.key {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidCursor));
        }
        let bind_start = binds.len() + 3;
        conditions.push(cursor_clause(&sort, bind_start, &compiled_sorts));
        for entry in &sort {
            binds.push(cursor_bind_value(
                entry.field,
                created_at,
                updated_at,
                number,
                &title,
                &sort_key,
                &priority,
                &status_sort_key,
                due,
            ));
        }
        binds.push(id.to_string());
    }

    let list_where_sql = conditions.join(" AND ");
    let count_where_sql = base_conditions.join(" AND ");
    let order_sql = order_clause(&sort, &compiled_sorts);
    let due_sort_sql = term_sql(SortField::Due, &compiled_sorts);
    let limit = query.limit + 1;
    let list_sql = format!(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate::text AS estimate,
               t.parent_id, t.milestone_id, t.sort_key, t.schema_version, t.version,
               t.archived_at, t.created_by, t.created_at, t.updated_at, t.recurrence,
               {due_sort_sql}::date AS sort_due,
               (
                   SELECT st.sort_key
                   FROM fvoci.statuses st
                   WHERE st.workspace_id = t.workspace_id
                     AND st.project_id = t.project_id
                     AND st.id = t.status_id
               ) AS status_sort_key
        FROM fvoci.tasks t
        WHERE {list_where_sql}
        ORDER BY {order_sql}
        LIMIT {limit}
        "#
    );
    let mut list_query = sqlx::query(&list_sql).bind(workspace_id).bind(scope_bind);
    for value in &binds {
        list_query = list_query.bind(value);
    }
    let rows = list_query.fetch_all(&mut *tx).await?;

    let count_sql = format!(
        r#"
        SELECT t.status_id, count(*)
        FROM fvoci.tasks t
        WHERE {count_where_sql}
        GROUP BY t.status_id
        "#
    );
    let mut count_query = sqlx::query_as::<_, (Uuid, i64)>(&count_sql)
        .bind(workspace_id)
        .bind(scope_bind);
    for value in &base_binds {
        count_query = count_query.bind(value);
    }
    let status_counts = count_query.fetch_all(&mut *tx).await?;

    let mut items = Vec::new();
    let mut item_ids = Vec::new();
    for row in rows.iter().take(query.limit as usize) {
        let record = map_task_row(row)?;
        item_ids.push(record.id);
        let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
        items.push(row_to_meta(workspace_id, record, recurrence));
    }
    let (assignee_map, label_map) = load_task_refs(&mut tx, workspace_id, &item_ids).await?;
    let items = items
        .into_iter()
        .map(|meta| {
            let id = meta.id;
            TaskListItemRow {
                assignee_ids: assignee_map.get(&id).cloned().unwrap_or_default(),
                label_ids: label_map.get(&id).cloned().unwrap_or_default(),
                meta,
            }
        })
        .collect();
    let next_cursor = if rows.len() as i32 > query.limit {
        let last = &rows[(query.limit - 1) as usize];
        let record = map_task_row(last)?;
        let created_at: DateTime<Utc> = last.try_get("created_at")?;
        let updated_at: DateTime<Utc> = last.try_get("updated_at")?;
        let due: Option<NaiveDate> = last.try_get("sort_due")?;
        let status_sort_key: String = last.try_get("status_sort_key")?;
        let tokens =
            load_custom_sort_tokens(&mut tx, workspace_id, record.id, &custom_fields).await?;
        let key = cursor_key_for_row(
            &sort,
            record.id,
            created_at,
            updated_at,
            record.number,
            &record.title,
            &record.sort_key,
            &record.priority,
            &status_sort_key,
            due,
            &tokens,
        );
        Some(encode_cursor(&TaskListCursor {
            id: record.id,
            key,
            f: fingerprint,
            as_of: query.as_of,
        }))
    } else {
        None
    };

    tx.commit().await?;
    Ok(Ok(TaskListPage {
        items,
        status_counts,
        next_cursor,
    }))
}

pub(crate) fn task_list_filter_conditions(
    query: &ParsedTaskListQuery,
    actor_user_id: Uuid,
    scope_condition: String,
) -> (Vec<String>, Vec<String>) {
    let mut binds: Vec<String> = Vec::new();
    let mut conditions = vec![
        "t.workspace_id = $1".to_string(),
        scope_condition,
        "t.deleted_at IS NULL".to_string(),
    ];
    if query.archived {
        conditions.push("t.archived_at IS NOT NULL".to_string());
    } else {
        conditions.push("t.archived_at IS NULL".to_string());
    }
    if let Some(task_type) = &query.view.filters.task_type {
        let idx = binds.len() + 3;
        binds.push(task_type.clone());
        conditions.push(format!("t.type = ${idx}"));
    }
    if let Some(status_id) = query.view.filters.status_id {
        let idx = binds.len() + 3;
        binds.push(status_id.to_string());
        conditions.push(format!("t.status_id = ${idx}::uuid"));
    }
    if let Some(priority) = &query.view.filters.priority {
        let idx = binds.len() + 3;
        binds.push(priority.clone());
        conditions.push(format!("t.priority = ${idx}"));
    }
    if query.view.filters.open_only {
        conditions.push(
            "EXISTS (
                SELECT 1 FROM fvoci.statuses s_open
                WHERE s_open.workspace_id = t.workspace_id
                  AND s_open.project_id = t.project_id
                  AND s_open.id = t.status_id
                  AND s_open.category NOT IN ('done', 'canceled')
            )"
            .to_string(),
        );
    }
    if let Some(title) = &query.view.filters.title {
        let idx = binds.len() + 3;
        binds.push(format!("%{}%", escape_ilike_pattern(title)));
        conditions.push(format!("t.title ILIKE ${idx} ESCAPE '\\'"));
    }
    if let Some(label_id) = query.view.filters.label_id {
        let idx = binds.len() + 3;
        binds.push(label_id.to_string());
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM fvoci.task_labels l
                WHERE l.workspace_id = t.workspace_id
                  AND l.task_id = t.id
                  AND l.label_id = ${idx}::uuid
            )"
        ));
    }
    if let Some(milestone_id) = query.view.filters.milestone_id {
        let idx = binds.len() + 3;
        binds.push(milestone_id.to_string());
        conditions.push(format!("t.milestone_id = ${idx}::uuid"));
    }
    if let Some(assignee) = &query.view.filters.assignee_id {
        let assignee_id = match assignee {
            AssigneeFilter::Me => actor_user_id,
            AssigneeFilter::User(id) => *id,
        };
        let idx = binds.len() + 3;
        binds.push(assignee_id.to_string());
        conditions.push(format!(
            "EXISTS (
                SELECT 1 FROM fvoci.task_assignees a
                WHERE a.workspace_id = t.workspace_id
                  AND a.task_id = t.id
                  AND a.user_id = ${idx}::uuid
            )"
        ));
    }
    if let (Some(from), Some(to)) = (query.from, query.to) {
        let from_idx = binds.len() + 3;
        binds.push(from.to_string());
        let to_idx = binds.len() + 3;
        binds.push(to.to_string());
        conditions.push(format!(
            "LEAST(t.start_date, COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)) <= ${to_idx}::date"
        ));
        conditions.push(format!(
            "GREATEST(t.start_date, COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date)) >= ${from_idx}::date"
        ));
    }
    let as_of_idx = binds.len() + 3;
    binds.push(query.as_of.to_rfc3339());
    conditions.push(format!("t.created_at <= ${as_of_idx}::timestamptz"));
    (conditions, binds)
}

/// Label/milestone filter target anywhere in the workspace (workspace-wide list).
async fn workspace_row_exists(
    tx: &mut Transaction<'_, Postgres>,
    table: &'static str,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let sql =
        format!("SELECT EXISTS (SELECT 1 FROM fvoci.{table} WHERE workspace_id = $1 AND id = $2)");
    sqlx::query_scalar(&sql)
        .bind(workspace_id)
        .bind(id)
        .fetch_one(&mut **tx)
        .await
}

fn escape_ilike_pattern(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Text of each field-sort value on one task (cursor key input).
async fn load_custom_sort_tokens(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
    fields: &[(Uuid, &'static str)],
) -> Result<Vec<(Uuid, Option<String>)>, sqlx::Error> {
    let mut out = Vec::with_capacity(fields.len());
    for (field_id, column) in fields {
        let expr = scalar_value_sql(RootKind::Task, "t", "$3", column);
        let value: Option<Option<String>> = sqlx::query_scalar(&format!(
            "SELECT ({expr})::text FROM fvoci.tasks t WHERE t.workspace_id = $1 AND t.id = $2"
        ))
        .bind(workspace_id)
        .bind(task_id)
        .bind(field_id.to_string())
        .fetch_optional(&mut **tx)
        .await?;
        out.push((*field_id, value.flatten()));
    }
    Ok(out)
}

/// Field and due sort terms from the shared compiler, keyed by sort field. The
/// due term reads `due_at` in the actor zone bound by the compiler.
pub(crate) fn compiled_sort_terms(compiled: &CompiledView) -> HashMap<SortField, String> {
    compiled
        .order
        .iter()
        .filter(|term| matches!(term.field, SortField::Field(_) | SortField::Due))
        .map(|term| (term.field, term.sql("{root}")))
        .collect()
}

fn term_sql(field: SortField, compiled: &HashMap<SortField, String>) -> String {
    match field {
        SortField::Field(_) | SortField::Due => compiled
            .get(&field)
            .map(|template| template.replace("{root}", "t"))
            .unwrap_or_else(|| "NULL".to_string()),
        other => sort_expression_sql(other).to_string(),
    }
}

fn sort_expression_sql(field: SortField) -> &'static str {
    match field {
        SortField::Field(_) | SortField::Due => "NULL",
        SortField::Priority => {
            "CASE t.priority WHEN 'none' THEN 0 WHEN 'low' THEN 1 WHEN 'medium' THEN 2 WHEN 'high' THEN 3 WHEN 'urgent' THEN 4 END"
        }
        SortField::Updated => "t.updated_at",
        SortField::Created => "t.created_at",
        SortField::Rank => r#"t.sort_key COLLATE "C""#,
        SortField::Title => r#"t.title COLLATE "C""#,
        SortField::Status => {
            "(SELECT st.sort_key FROM fvoci.statuses st WHERE st.workspace_id = t.workspace_id AND st.project_id = t.project_id AND st.id = t.status_id)"
        }
        SortField::Number => "t.number",
    }
}

fn sort_anchor_ref(field: SortField, bind_index: usize) -> String {
    match field {
        SortField::Created | SortField::Updated => format!("${bind_index}::timestamptz"),
        SortField::Number | SortField::Priority => format!("${bind_index}::int"),
        SortField::Due => format!("NULLIF(${bind_index}, 'null')::date"),
        SortField::Title | SortField::Rank | SortField::Status | SortField::Field(_) => {
            format!("${bind_index}")
        }
    }
}

pub(crate) fn order_clause(sort: &[ViewSort], custom: &HashMap<SortField, String>) -> String {
    let mut parts = Vec::new();
    for entry in sort {
        let column = term_sql(entry.field, custom);
        let dir = if entry.direction == SortDirection::Asc {
            "ASC"
        } else {
            "DESC"
        };
        parts.push(format!("{column} {dir} NULLS LAST"));
    }
    parts.push("t.id ASC".to_string());
    parts.join(", ")
}

fn cursor_clause(
    sort: &[ViewSort],
    bind_start: usize,
    custom: &HashMap<SortField, String>,
) -> String {
    let id_bind = bind_start + sort.len();
    let mut branches = Vec::with_capacity(sort.len() + 1);
    for (index, entry) in sort.iter().enumerate() {
        let mut parts = Vec::with_capacity(index + 1);
        for (prior_index, prior) in sort[..index].iter().enumerate() {
            parts.push(sort_equality_sql(
                prior.field,
                bind_start + prior_index,
                id_bind,
                custom,
            ));
        }
        parts.push(sort_strict_after_sql(
            entry.field,
            bind_start + index,
            entry.direction,
            id_bind,
            custom,
        ));
        branches.push(format!("({})", parts.join(" AND ")));
    }
    let mut equal_parts: Vec<String> = sort
        .iter()
        .enumerate()
        .map(|(index, entry)| sort_equality_sql(entry.field, bind_start + index, id_bind, custom))
        .collect();
    equal_parts.push(format!("t.id > ${id_bind}::uuid"));
    branches.push(format!("({})", equal_parts.join(" AND ")));
    format!("({})", branches.join(" OR "))
}

/// Field sorts compare against the anchor row's own value (re-evaluated in
/// SQL); built-in sorts compare against the bound anchor token.
fn anchor_sql(
    field: SortField,
    bind_index: usize,
    id_bind: usize,
    custom: &HashMap<SortField, String>,
) -> String {
    match field {
        SortField::Field(_) => match custom.get(&field) {
            Some(template) => format!(
                "(SELECT {} FROM fvoci.tasks a WHERE a.workspace_id = $1 AND a.id = ${id_bind}::uuid)",
                template.replace("{root}", "a")
            ),
            None => "NULL".to_string(),
        },
        other => sort_anchor_ref(other, bind_index),
    }
}

fn sort_equality_sql(
    field: SortField,
    bind_index: usize,
    id_bind: usize,
    custom: &HashMap<SortField, String>,
) -> String {
    let expr = term_sql(field, custom);
    let anchor = anchor_sql(field, bind_index, id_bind, custom);
    format!("{expr} IS NOT DISTINCT FROM {anchor}")
}

fn sort_strict_after_sql(
    field: SortField,
    bind_index: usize,
    direction: SortDirection,
    id_bind: usize,
    custom: &HashMap<SortField, String>,
) -> String {
    let expr = term_sql(field, custom);
    let anchor = anchor_sql(field, bind_index, id_bind, custom);
    let op = if direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    format!("({expr} IS NULL AND {anchor} IS NOT NULL OR {expr} {op} {anchor})")
}

#[allow(clippy::too_many_arguments)]
fn cursor_bind_value(
    field: SortField,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    number: i32,
    title: &str,
    sort_key: &str,
    priority: &str,
    status_sort_key: &str,
    due: Option<NaiveDate>,
) -> String {
    sort_value_token(
        field,
        created_at,
        updated_at,
        number,
        title,
        sort_key,
        priority,
        status_sort_key,
        due,
    )
}

#[derive(Debug, Clone)]
pub(crate) struct TaskWriteRow {
    pub(crate) record: TaskRowRecord,
    pub(crate) recurrence: Option<Value>,
}

async fn task_project_id_for_write(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(project_id,)| project_id))
}

async fn load_task_for_write_locked(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskWriteRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, project_id, number, title, type AS task_type, priority, status_id,
               start_date, due_date, due_at, estimate::text AS estimate, parent_id,
               milestone_id, sort_key, schema_version, version, archived_at, created_by, created_at,
               updated_at, recurrence
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let record = map_task_row(&row)?;
    let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
    Ok(Some(TaskWriteRow { record, recurrence }))
}

pub(crate) async fn require_task_write_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    allow_archived: bool,
) -> Result<Result<TaskWriteRow, ProjectDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(project_id) = task_project_id_for_write(tx, workspace_id, task_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    let locked = lock_project(tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(task) = load_task_for_write_locked(tx, workspace_id, task_id).await? else {
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !allow_archived && task.record.archived_at.is_some() {
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    Ok(Ok(task))
}

fn resolve_anchor_sort_key(
    ordered: &[(Uuid, String)],
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
) -> Result<String, ProjectDbError> {
    if before_id.is_some() && after_id.is_some() {
        return Err(ProjectDbError::InvalidMoveAnchors);
    }
    if let Some(before_id) = before_id {
        let index = ordered.iter().position(|(id, _)| *id == before_id);
        let Some(index) = index else {
            return Err(ProjectDbError::InvalidAnchor);
        };
        let left = index
            .checked_sub(1)
            .and_then(|i| ordered.get(i))
            .map(|(_, key)| key.as_str());
        let right = ordered.get(index).map(|(_, key)| key.as_str());
        return between(left, right).map_err(|_| ProjectDbError::InvalidAnchor);
    }
    if let Some(after_id) = after_id {
        let index = ordered.iter().position(|(id, _)| *id == after_id);
        let Some(index) = index else {
            return Err(ProjectDbError::InvalidAnchor);
        };
        let left = ordered.get(index).map(|(_, key)| key.as_str());
        let right = ordered.get(index + 1).map(|(_, key)| key.as_str());
        return between(left, right).map_err(|_| ProjectDbError::InvalidAnchor);
    }
    let last = ordered.last().map(|(_, key)| key.as_str());
    between(last, None).map_err(|_| ProjectDbError::InvalidAnchor)
}

async fn list_sorted_tasks_in_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    status_id: Uuid,
    exclude_id: Option<Uuid>,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT id, sort_key
        FROM fvoci.tasks
        WHERE workspace_id = $1
          AND status_id = $2
          AND deleted_at IS NULL
          AND archived_at IS NULL
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(status_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(id, _)| exclude_id.map(|skip| skip != *id).unwrap_or(true))
        .collect())
}

#[derive(Debug, Clone, Copy)]
struct TransitionResult {
    status_changed: bool,
}

#[derive(Debug, Clone)]
struct RecurrenceSpawnFields {
    task_type: String,
    parent_id: Option<Uuid>,
}

async fn spawn_recurring_next_task(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskWriteRow,
    recurrence_kind: &str,
    spawn_fields: Option<&RecurrenceSpawnFields>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let next_id = Uuid::now_v7();
    let number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    let target_status = match default_backlog_status(tx, workspace_id, project_id).await? {
        Some(status_id) => status_id,
        None => return Ok(Err(ProjectDbError::WorkflowHasNoStatuses)),
    };
    let siblings = list_sorted_tasks_in_status(tx, workspace_id, target_status, None).await?;
    let sort_key = match resolve_anchor_sort_key(&siblings, None, None) {
        Ok(key) => key,
        Err(err) => return Ok(Err(err)),
    };
    let task_type = spawn_fields
        .map(|fields| fields.task_type.as_str())
        .unwrap_or(&task.record.task_type);
    let parent_id = spawn_fields
        .map(|fields| fields.parent_id)
        .unwrap_or(task.record.parent_id);
    let shift = |date: Option<NaiveDate>| match date {
        None => Ok(None),
        Some(date) => shift_recurrence_date(date, recurrence_kind)
            .map(Some)
            .ok_or_else(|| sqlx::Error::Protocol("recurrence date out of range".into())),
    };
    let next_start = shift(task.record.start_date)?;
    let next_due = shift(task.record.due_date)?;
    let recurrence = json!({ "kind": recurrence_kind });
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            start_date, due_date, parent_id, milestone_id, recurrence, sort_key,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17
        )
        "#,
    )
    .bind(next_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(number.0)
    .bind(&task.record.title)
    .bind(task_type)
    .bind(&task.record.priority)
    .bind(target_status)
    .bind(next_start)
    .bind(next_due)
    .bind(parent_id)
    .bind(task.record.milestone_id)
    .bind(recurrence)
    .bind(sort_key)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    copy_task_assignees_and_labels(tx, workspace_id, task.record.id, next_id).await?;
    record_task_event_and_audit(
        tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.created",
            target_type: "task",
            target_id: next_id,
            payload: json!({
                "taskId": next_id.to_string(),
                "projectId": project_id.to_string(),
                "number": number.0,
                "statusId": target_status.to_string(),
                "title": task.record.title,
                "recurrenceOf": task.record.id.to_string(),
            }),
            client_ip: None,
        },
    )
    .await?;
    record_task_activity(
        tx,
        workspace_id,
        next_id,
        actor_user_id,
        "system",
        None,
        &ActivitySnapshot::new(),
    )
    .await?;
    Ok(Ok(()))
}

#[allow(clippy::too_many_arguments)]
async fn transition_task_status(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskWriteRow,
    from_status_id: Uuid,
    to_status_id: Uuid,
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
    spawn_fields: Option<&RecurrenceSpawnFields>,
) -> Result<Result<TransitionResult, ProjectDbError>, sqlx::Error> {
    if from_status_id == to_status_id && before_id.is_none() && after_id.is_none() {
        return Ok(Ok(TransitionResult {
            status_changed: false,
        }));
    }
    let to_status = load_status_info(tx, workspace_id, project_id, to_status_id).await?;
    let Some(to_status) = to_status else {
        return Ok(Err(ProjectDbError::StatusNotInWorkflow));
    };
    let from_status = load_status_info(tx, workspace_id, project_id, from_status_id).await?;
    let changing_column = from_status_id != to_status_id;
    if changing_column && to_status.wip_limit.is_some() {
        lock_status_for_transition(tx, to_status_id).await?;
        let occupied = count_active_tasks_in_status(tx, workspace_id, to_status_id).await?;
        let limit = to_status.wip_limit.unwrap_or(0) as i64;
        if occupied >= limit {
            return Ok(Err(ProjectDbError::WipLimitExceeded));
        }
    }
    let siblings =
        list_sorted_tasks_in_status(tx, workspace_id, to_status_id, Some(task.record.id)).await?;
    let sort_key = match resolve_anchor_sort_key(&siblings, before_id, after_id) {
        Ok(key) => key,
        Err(err) => return Ok(Err(err)),
    };
    let result = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET status_id = $4, sort_key = $5, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task.record.id)
    .bind(from_status_id)
    .bind(to_status_id)
    .bind(&sort_key)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 0 {
        return Ok(Err(ProjectDbError::VersionConflict));
    }
    if changing_column
        && to_status.category == "done"
        && from_status.as_ref().map(|s| s.category.as_str()) != Some("done")
    {
        if let Some(kind) = task.recurrence.as_ref().and_then(parse_recurrence_kind) {
            sqlx::query(
                r#"
                UPDATE fvoci.tasks
                SET recurrence = NULL, updated_at = now()
                WHERE workspace_id = $1 AND id = $2
                "#,
            )
            .bind(workspace_id)
            .bind(task.record.id)
            .execute(&mut **tx)
            .await?;
            match spawn_recurring_next_task(
                tx,
                workspace_id,
                project_id,
                actor_user_id,
                task,
                kind,
                spawn_fields,
            )
            .await?
            {
                Ok(()) => {}
                Err(err) => return Ok(Err(err)),
            }
        }
    }
    Ok(Ok(TransitionResult {
        status_changed: changing_column,
    }))
}

/// `dueAt` is compared to the millisecond, not to the microsecond PostgreSQL
/// stores: browser clients hold it in a JS `Date`, and the task layout and
/// collection rows render it with milliseconds. A different millisecond is
/// still a conflict.
fn dates_conflict(
    expected: &crate::tasks::patch::ExpectedDatesInput,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    due_at: Option<DateTime<Utc>>,
) -> bool {
    let millis = |at: Option<DateTime<Utc>>| at.map(|at| at.timestamp_millis());
    expected.start_date != start_date
        || expected.due_date != due_date
        || millis(expected.due_at) != millis(due_at)
}

fn patch_only_unarchives(input: &crate::tasks::patch::PatchTaskMetaInput) -> bool {
    input.archived == Some(false)
        && input.task_type.is_none()
        && input.title.is_none()
        && input.priority.is_none()
        && input.status_id.is_none()
        && input.start_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_at == crate::tasks::patch::FieldUpdate::Unchanged
        && input.estimate == crate::tasks::patch::FieldUpdate::Unchanged
        && input.parent_id == crate::tasks::patch::FieldUpdate::Unchanged
        && input.milestone_id == crate::tasks::patch::FieldUpdate::Unchanged
        && input.recurrence == crate::tasks::patch::FieldUpdate::Unchanged
        && input.expected_dates.is_none()
        && input.assignee_ids.is_none()
        && input.label_ids.is_none()
}

#[allow(clippy::too_many_arguments)]
pub async fn patch_task_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: crate::tasks::patch::PatchTaskMetaInput,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    let restores_archived = patch_only_unarchives(&input);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    // Membership locks come before the project row lock on every write path; take
    // the assignees' together with the actor's (sorted) before anything else.
    if let Some(assignee_ids) = input
        .assignee_ids
        .as_deref()
        .filter(|ids| !ids.is_empty() && ids.len() <= MAX_TASK_REFS)
    {
        let mut lock_ids = unique_ids(assignee_ids);
        lock_ids.push(actor_user_id);
        lock_membership_users(&mut tx, &lock_ids).await?;
    }
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        restores_archived,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if let Some(expected) = &input.expected_dates {
        if dates_conflict(
            expected,
            task.record.start_date,
            task.record.due_date,
            task.record.due_at,
        ) {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::VersionConflict));
        }
    }

    let activity_fields = patch_activity_fields(&input);
    let before_activity = if activity_fields.is_empty() {
        None
    } else {
        Some(
            task_activity_snapshot(
                &mut tx,
                workspace_id,
                &task.record,
                task.recurrence.as_ref(),
                &activity_fields,
            )
            .await?,
        )
    };

    let next_type = input.task_type.as_deref().unwrap_or(&task.record.task_type);
    let next_parent = match input.parent_id {
        crate::tasks::patch::FieldUpdate::Set(parent_id) => Some(parent_id),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => task.record.parent_id,
    };
    let parent_changed = input.parent_id != crate::tasks::patch::FieldUpdate::Unchanged;
    if input.task_type.is_some() || parent_changed {
        match assert_task_hierarchy(
            &mut tx,
            workspace_id,
            task.record.project_id,
            task_id,
            next_type,
            next_parent,
            parent_changed,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    let recurrence_spawn_fields = RecurrenceSpawnFields {
        task_type: next_type.to_string(),
        parent_id: next_parent,
    };

    if input.milestone_id != crate::tasks::patch::FieldUpdate::Unchanged {
        if let crate::tasks::patch::FieldUpdate::Set(milestone_id) = input.milestone_id {
            if !project_milestone_exists(
                &mut tx,
                workspace_id,
                task.record.project_id,
                milestone_id,
            )
            .await?
            {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::MilestoneNotFound));
            }
        }
    }

    if input.start_date != crate::tasks::patch::FieldUpdate::Unchanged
        || input.due_date != crate::tasks::patch::FieldUpdate::Unchanged
        || input.due_at != crate::tasks::patch::FieldUpdate::Unchanged
    {
        match assert_dependency_dates_ok(&mut tx, workspace_id, &task.record, &input).await? {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    let mut status_changed = false;
    let from_status_id = task.record.status_id;
    if let Some(status_id) = input.status_id {
        if status_id != task.record.status_id {
            let result = transition_task_status(
                &mut tx,
                workspace_id,
                task.record.project_id,
                actor_user_id,
                &task,
                task.record.status_id,
                status_id,
                None,
                None,
                Some(&recurrence_spawn_fields),
            )
            .await?;
            match result {
                Ok(transition) => status_changed = transition.status_changed,
                Err(err) => {
                    tx.rollback().await?;
                    return Ok(Err(err));
                }
            }
        }
    }

    let mut sets = Vec::<String>::new();
    let mut bind_idx = 3u32;
    let mut bind_title: Option<String> = None;
    let mut bind_type: Option<String> = None;
    let mut bind_priority: Option<String> = None;
    let mut bind_start_date: Option<Option<NaiveDate>> = None;
    let mut bind_due_date: Option<Option<NaiveDate>> = None;
    let mut bind_due_at: Option<Option<DateTime<Utc>>> = None;
    let mut bind_estimate: Option<Option<String>> = None;
    let mut bind_parent_id: Option<Option<Uuid>> = None;
    let mut bind_milestone_id: Option<Option<Uuid>> = None;
    let mut bind_recurrence: Option<Option<Value>> = None;
    let mut bind_archived_at: Option<Option<DateTime<Utc>>> = None;

    // The project stream filters task verbs by `projectId`; take it from the
    // authorized, locked task row, never from the request.
    let mut payload = serde_json::Map::new();
    payload.insert("taskId".to_string(), json!(task_id.to_string()));
    payload.insert(
        "projectId".to_string(),
        json!(task.record.project_id.to_string()),
    );

    if let Some(title) = &input.title {
        sets.push(format!("title = ${bind_idx}"));
        bind_idx += 1;
        bind_title = Some(title.clone());
        payload.insert("title".to_string(), json!(title));
    }
    if let Some(task_type) = &input.task_type {
        sets.push(format!("type = ${bind_idx}"));
        bind_idx += 1;
        bind_type = Some(task_type.clone());
        payload.insert("type".to_string(), json!(task_type));
    }
    if let Some(priority) = &input.priority {
        sets.push(format!("priority = ${bind_idx}"));
        bind_idx += 1;
        bind_priority = Some(priority.clone());
        payload.insert("priority".to_string(), json!(priority));
    }
    if input.start_date != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("start_date = ${bind_idx}"));
        bind_idx += 1;
        bind_start_date = Some(match input.start_date {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "startDate".to_string(),
            json!(bind_start_date.as_ref().unwrap()),
        );
    }
    if input.due_date != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("due_date = ${bind_idx}"));
        bind_idx += 1;
        bind_due_date = Some(match input.due_date {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "dueDate".to_string(),
            json!(bind_due_date.as_ref().unwrap()),
        );
    }
    if input.due_at != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("due_at = ${bind_idx}"));
        bind_idx += 1;
        bind_due_at = Some(match input.due_at {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert("dueAt".to_string(), json!(bind_due_at.as_ref().unwrap()));
    }
    if input.estimate != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("estimate = ${bind_idx}::numeric"));
        // This existing generic field does not declare a time unit.
        sets.push("estimate_unit = NULL".to_string());
        bind_idx += 1;
        bind_estimate = Some(match &input.estimate {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value.clone()),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "estimate".to_string(),
            json!(bind_estimate.as_ref().unwrap()),
        );
    }
    if input.parent_id != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("parent_id = ${bind_idx}"));
        bind_idx += 1;
        bind_parent_id = Some(match input.parent_id {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "parentId".to_string(),
            json!(bind_parent_id.as_ref().unwrap().map(|id| id.to_string())),
        );
    }
    if input.milestone_id != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("milestone_id = ${bind_idx}"));
        bind_idx += 1;
        bind_milestone_id = Some(match input.milestone_id {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "milestoneId".to_string(),
            json!(bind_milestone_id.as_ref().unwrap().map(|id| id.to_string())),
        );
    }
    if input.recurrence != crate::tasks::patch::FieldUpdate::Unchanged {
        sets.push(format!("recurrence = ${bind_idx}"));
        bind_idx += 1;
        bind_recurrence = Some(match &input.recurrence {
            crate::tasks::patch::FieldUpdate::Clear => None,
            crate::tasks::patch::FieldUpdate::Set(value) => Some(value.clone()),
            crate::tasks::patch::FieldUpdate::Unchanged => unreachable!(),
        });
        payload.insert(
            "recurrence".to_string(),
            json!(bind_recurrence.as_ref().unwrap()),
        );
    }
    if let Some(archived) = input.archived {
        sets.push(format!("archived_at = ${bind_idx}"));
        bind_archived_at = Some(if archived { Some(Utc::now()) } else { None });
        payload.insert("archived".to_string(), json!(archived));
    }

    let meta_changed = !sets.is_empty();
    let row = if meta_changed {
        sets.push("updated_at = now()".to_string());
        let sql = format!(
            r#"
            UPDATE fvoci.tasks
            SET {}
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
            RETURNING id, project_id, number, title, type AS task_type, priority, status_id,
                      start_date, due_date, due_at, estimate::text AS estimate,
                      parent_id, milestone_id, sort_key, schema_version, version, archived_at,
                      created_by, created_at, updated_at
            "#,
            sets.join(", ")
        );
        let mut query = sqlx::query(&sql).bind(workspace_id).bind(task_id);
        if let Some(title) = bind_title {
            query = query.bind(title);
        }
        if let Some(task_type) = bind_type {
            query = query.bind(task_type);
        }
        if let Some(priority) = bind_priority {
            query = query.bind(priority);
        }
        if let Some(start_date) = bind_start_date {
            query = query.bind(start_date);
        }
        if let Some(due_date) = bind_due_date {
            query = query.bind(due_date);
        }
        if let Some(due_at) = bind_due_at {
            query = query.bind(due_at);
        }
        if let Some(estimate) = bind_estimate {
            query = query.bind(estimate);
        }
        if let Some(parent_id) = bind_parent_id {
            query = query.bind(parent_id);
        }
        if let Some(milestone_id) = bind_milestone_id {
            query = query.bind(milestone_id);
        }
        if let Some(recurrence) = bind_recurrence {
            query = query.bind(recurrence);
        }
        if let Some(archived_at) = bind_archived_at {
            query = query.bind(archived_at);
        }
        let row = query.fetch_optional(&mut *tx).await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        map_task_row(&row)?
    } else if status_changed || input.assignee_ids.is_some() || input.label_ids.is_some() {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, number, title, type AS task_type, priority, status_id,
                   start_date, due_date, due_at, estimate::text AS estimate, parent_id,
                   milestone_id, sort_key, schema_version, version, archived_at, created_by,
                   created_at, updated_at
            FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
            "#,
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        };
        map_task_row(&row)?
    } else {
        tx.rollback().await?;
        return Ok(Ok(row_to_meta(workspace_id, task.record, task.recurrence)));
    };

    let recurrence = load_task_recurrence(&mut tx, workspace_id, task_id).await?;
    if status_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "projectId": task.record.project_id.to_string(),
                    "from": from_status_id.to_string(),
                    "to": row.status_id.to_string(),
                }),
                client_ip,
            },
        )
        .await?;
    }
    if meta_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: Value::Object(payload),
                client_ip,
            },
        )
        .await?;
    }
    if let Some(assignee_ids) = &input.assignee_ids {
        match replace_task_assignees(
            &mut tx,
            workspace_id,
            actor_user_id,
            row.project_id,
            task_id,
            assignee_ids,
            client_ip,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }
    if let Some(label_ids) = &input.label_ids {
        match replace_task_labels(
            &mut tx,
            workspace_id,
            actor_user_id,
            row.project_id,
            task_id,
            label_ids,
            client_ip,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
    }

    if let Some(before_activity) = before_activity {
        let after_activity = task_activity_snapshot(
            &mut tx,
            workspace_id,
            &row,
            recurrence.as_ref(),
            &activity_fields,
        )
        .await?;
        record_task_activity(
            &mut tx,
            workspace_id,
            task_id,
            actor_user_id,
            channel,
            Some(&before_activity),
            &after_activity,
        )
        .await?;
    }

    tx.commit().await?;
    Ok(Ok(row_to_meta(workspace_id, row, recurrence)))
}

#[allow(clippy::too_many_arguments)]
pub async fn move_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: crate::tasks::patch::MoveTaskInput,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<TaskMetaRow, ProjectDbError>, sqlx::Error> {
    if input.before_id.is_some() && input.after_id.is_some() {
        return Ok(Err(ProjectDbError::InvalidMoveAnchors));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if let Some(expected) = input.expected_status_id {
        if expected != task.record.status_id {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::VersionConflict));
        }
    }
    let activity_fields = ["statusId", "recurrence"];
    let before_activity = task_activity_snapshot(
        &mut tx,
        workspace_id,
        &task.record,
        task.recurrence.as_ref(),
        &activity_fields,
    )
    .await?;
    let from_status_id = task.record.status_id;
    let result = transition_task_status(
        &mut tx,
        workspace_id,
        task.record.project_id,
        actor_user_id,
        &task,
        task.record.status_id,
        input.status_id,
        input.before_id,
        input.after_id,
        None,
    )
    .await?;
    let transition = match result {
        Ok(value) => value,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if transition.status_changed {
        record_task_event_and_audit(
            &mut tx,
            TaskChangeRecord {
                workspace_id,
                actor_user_id,
                verb: "task.updated",
                target_type: "task",
                target_id: task_id,
                payload: json!({
                    "taskId": task_id.to_string(),
                    "projectId": task.record.project_id.to_string(),
                    "from": from_status_id.to_string(),
                    "to": input.status_id.to_string(),
                }),
                client_ip,
            },
        )
        .await?;
    }
    let row = sqlx::query(
        r#"
        SELECT id, project_id, number, title, type AS task_type, priority, status_id,
               start_date, due_date, due_at, estimate::text AS estimate,
               parent_id, milestone_id, sort_key, schema_version, version, archived_at,
               created_by, created_at, updated_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let row = map_task_row(&row)?;
    let recurrence = load_task_recurrence(&mut tx, workspace_id, task_id).await?;
    let after_activity = task_activity_snapshot(
        &mut tx,
        workspace_id,
        &row,
        recurrence.as_ref(),
        &activity_fields,
    )
    .await?;
    record_task_activity(
        &mut tx,
        workspace_id,
        task_id,
        actor_user_id,
        channel,
        Some(&before_activity),
        &after_activity,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(row_to_meta(workspace_id, row, recurrence)))
}

pub async fn trash_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let task = match require_task_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        false,
    )
    .await?
    {
        Ok(task) => task,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET deleted_at = now(), updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let assignee_ids = list_task_assignee_ids(&mut tx, workspace_id, task_id).await?;
    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.deleted",
            target_type: "task",
            target_id: task_id,
            payload: json!({
                "taskId": task_id.to_string(),
                "projectId": task.record.project_id.to_string(),
                "number": task.record.number,
                "assigneeIds": uuid_strings(&assignee_ids),
            }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn restore_task(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT project_id
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((project_id,)) = row else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    let permission = project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(ProjectPermission::Edit) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.tasks
        SET deleted_at = NULL, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    record_task_event_and_audit(
        &mut tx,
        TaskChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "task.restored",
            target_type: "task",
            target_id: task_id,
            payload: json!({ "taskId": task_id.to_string() }),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

async fn lock_tasks_sorted(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut sorted = ids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for id in sorted {
        sqlx::query(
            r#"
            SELECT id FROM fvoci.tasks
            WHERE workspace_id = $1 AND id = $2
            FOR NO KEY UPDATE
            "#,
        )
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
    }
    Ok(())
}

type DependencySqlRow = (Uuid, Uuid, String, i32);

fn map_dependency_row(
    blocker_id: Uuid,
    blocked_id: Uuid,
    dependency_type: String,
    lag_days: i32,
) -> TaskDependencyEdge {
    TaskDependencyEdge {
        blocker_id,
        blocked_id,
        dependency_type,
        lag_days,
    }
}

async fn list_task_dependency_edges(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Vec<TaskDependencyEdge>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DependencySqlRow>(
        r#"
        SELECT blocker_id, blocked_id, type, lag_days
        FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND (blocker_id = $2 OR blocked_id = $2)
        ORDER BY blocker_id, blocked_id
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(blocker_id, blocked_id, dependency_type, lag_days)| {
            map_dependency_row(blocker_id, blocked_id, dependency_type, lag_days)
        })
        .collect())
}

async fn inspect_addition_creates_cycle(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let creates: (bool,) = sqlx::query_as(
        r#"
        WITH RECURSIVE reachable(id) AS (
            VALUES ($4::uuid)
            UNION
            SELECT d.blocked_id
            FROM fvoci.task_dependencies d
            INNER JOIN reachable r ON r.id = d.blocker_id
            INNER JOIN fvoci.tasks t
                ON t.id = d.blocker_id AND t.workspace_id = d.workspace_id
            WHERE d.workspace_id = $1 AND t.project_id = $2
        )
        SELECT EXISTS(SELECT 1 FROM reachable WHERE id = $3)
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(creates.0)
}

async fn load_task_schedule(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<(Uuid, Option<DateTime<Utc>>, ScheduleEnds)>, sqlx::Error> {
    type ScheduleSqlRow = (
        Uuid,
        Option<DateTime<Utc>>,
        Option<NaiveDate>,
        Option<NaiveDate>,
        Option<DateTime<Utc>>,
    );
    let row: Option<ScheduleSqlRow> = sqlx::query_as(
        r#"
        SELECT project_id, archived_at, start_date, due_date, due_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(
        row.map(|(project_id, archived_at, start_date, due_date, due_at)| {
            (
                project_id,
                archived_at,
                schedule_ends(start_date, due_date, due_at),
            )
        }),
    )
}

fn merged_schedule_ends(
    record: &TaskRowRecord,
    input: &crate::tasks::patch::PatchTaskMetaInput,
) -> ScheduleEnds {
    let start_date = match input.start_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.start_date,
    };
    let due_date = match input.due_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_date,
    };
    let due_at = match input.due_at {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_at,
    };
    ScheduleEnds {
        start_date,
        due_date: finish_date(due_date, due_at),
    }
}

async fn assert_dependency_dates_ok(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    record: &TaskRowRecord,
    input: &crate::tasks::patch::PatchTaskMetaInput,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let due_after = match input.due_date {
        crate::tasks::patch::FieldUpdate::Set(value) => Some(value),
        crate::tasks::patch::FieldUpdate::Clear => None,
        crate::tasks::patch::FieldUpdate::Unchanged => record.due_date,
    };
    if input.start_date == crate::tasks::patch::FieldUpdate::Unchanged
        && input.due_date == crate::tasks::patch::FieldUpdate::Unchanged
        && due_after.is_some()
    {
        return Ok(Ok(()));
    }
    let holidays = list_holiday_dates(tx, workspace_id).await?;
    let merged = merged_schedule_ends(record, input);
    let edges = list_task_dependency_edges(tx, workspace_id, record.id).await?;
    for edge in edges {
        let other_id = if edge.blocker_id == record.id {
            edge.blocked_id
        } else {
            edge.blocker_id
        };
        let Some((_, _, other_ends)) = load_task_schedule(tx, workspace_id, other_id).await? else {
            continue;
        };
        let Some(dep_type) = DependencyType::parse(&edge.dependency_type) else {
            continue;
        };
        let blocker_dates = if edge.blocker_id == record.id {
            merged
        } else {
            other_ends
        };
        let blocked_dates = if edge.blocked_id == record.id {
            merged
        } else {
            other_ends
        };
        if required_dates_present(dep_type, blocker_dates, blocked_dates)
            && violates_inequality(
                dep_type,
                edge.lag_days,
                blocker_dates,
                blocked_dates,
                &holidays,
            )
        {
            return Ok(Err(ProjectDbError::DependencyContradiction));
        }
    }
    Ok(Ok(()))
}

pub async fn list_project_dependencies(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TaskDependencyEdge>, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let Some(project) = load_live_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, DependencySqlRow>(
        r#"
        SELECT d.blocker_id, d.blocked_id, d.type, d.lag_days
        FROM fvoci.task_dependencies d
        INNER JOIN fvoci.tasks t
            ON t.workspace_id = d.workspace_id AND t.id = d.blocker_id
        WHERE d.workspace_id = $1 AND t.project_id = $2
        ORDER BY d.blocker_id, d.blocked_id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(blocker_id, blocked_id, dependency_type, lag_days)| {
            map_dependency_row(blocker_id, blocked_id, dependency_type, lag_days)
        })
        .collect()))
}

#[allow(clippy::too_many_arguments)]
pub async fn add_task_dependency(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
    requested_type: Option<DependencyType>,
    requested_lag: Option<i32>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if blocker_id == blocked_id {
        return Ok(Err(ProjectDbError::TaskCannotBlockItself));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let blocker_project: Option<(Uuid,)> =
        sqlx::query_as("SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(blocker_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((project_id,)) = blocker_project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tasks_sorted(&mut tx, workspace_id, &[blocker_id, blocked_id]).await?;
    let Some((blocker_project_id, blocker_archived, blocker_ends)) =
        load_task_schedule(&mut tx, workspace_id, blocker_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some((blocked_project_id, blocked_archived, blocked_ends)) =
        load_task_schedule(&mut tx, workspace_id, blocked_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if blocker_project_id != blocked_project_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    if blocker_archived.is_some() || blocked_archived.is_some() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    if inspect_addition_creates_cycle(
        &mut tx,
        workspace_id,
        blocker_project_id,
        blocker_id,
        blocked_id,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyCycle));
    }
    let existing: Option<(String, i32)> = sqlx::query_as(
        r#"
        SELECT type, lag_days FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND blocker_id = $2 AND blocked_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .fetch_optional(&mut *tx)
    .await?;
    let dep_type = requested_type
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|(value, _)| DependencyType::parse(value))
        })
        .unwrap_or(DependencyType::Fs);
    let lag_days = requested_lag.or(existing.map(|(_, lag)| lag)).unwrap_or(0);
    let holidays = list_holiday_dates(&mut tx, workspace_id).await?;
    if required_dates_present(dep_type, blocker_ends, blocked_ends)
        && violates_inequality(dep_type, lag_days, blocker_ends, blocked_ends, &holidays)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyContradiction));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_dependencies (workspace_id, blocker_id, blocked_id, type, lag_days)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (blocker_id, blocked_id) DO UPDATE
        SET type = EXCLUDED.type, lag_days = EXCLUDED.lag_days
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .bind(dep_type.as_str())
    .bind(lag_days)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_task_dependency(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    blocker_id: Uuid,
    blocked_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let blocker_project: Option<(Uuid,)> =
        sqlx::query_as("SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(blocker_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((project_id,)) = blocker_project else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let Some(locked) = lock_project(&mut tx, workspace_id, project_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tasks_sorted(&mut tx, workspace_id, &[blocker_id, blocked_id]).await?;
    let Some((blocker_project_id, blocker_archived, _)) =
        load_task_schedule(&mut tx, workspace_id, blocker_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if blocker_archived.is_some() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    let Some((blocked_project_id, blocked_archived, _)) =
        load_task_schedule(&mut tx, workspace_id, blocked_id).await?
    else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    };
    if blocker_project_id != blocked_project_id {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    }
    if blocked_archived.is_some() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::TaskArchived));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.task_dependencies
        WHERE workspace_id = $1 AND blocker_id = $2 AND blocked_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(blocker_id)
    .bind(blocked_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::DependencyNotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source dashboard `listTasks(OPEN_ASSIGNED_QUERY, limit)` for one workspace:
/// open tasks assigned to `user_id` in `project_ids`, due ascending (nulls
/// last), id ascending. Also returns the due day in `time_zone` for the
/// cross-workspace merge. The caller's transaction holds the tenant context.
pub(crate) async fn list_open_assigned_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    project_ids: &[Uuid],
    time_zone: &str,
    limit: i64,
) -> Result<Vec<(TaskListItemRow, Option<NaiveDate>)>, sqlx::Error> {
    if project_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.project_id, t.number, t.title, t.type AS task_type, t.priority, t.status_id,
               t.start_date, t.due_date, t.due_at, t.estimate::text AS estimate,
               t.parent_id, t.milestone_id, t.sort_key, t.schema_version, t.version,
               t.archived_at, t.created_by, t.created_at, t.updated_at, t.recurrence,
               COALESCE(t.due_date, (t.due_at AT TIME ZONE $4)::date) AS due_key
        FROM fvoci.tasks t
        WHERE t.workspace_id = $1
          AND t.project_id = ANY($3)
          AND t.deleted_at IS NULL
          AND t.archived_at IS NULL
          AND EXISTS (
            SELECT 1 FROM fvoci.task_assignees a
            WHERE a.workspace_id = t.workspace_id AND a.task_id = t.id AND a.user_id = $2
          )
          AND EXISTS (
            SELECT 1 FROM fvoci.statuses s_open
            WHERE s_open.workspace_id = t.workspace_id
              AND s_open.project_id = t.project_id
              AND s_open.id = t.status_id
              AND s_open.category NOT IN ('done', 'canceled')
          )
        ORDER BY COALESCE(t.due_date, (t.due_at AT TIME ZONE 'UTC')::date) ASC NULLS LAST, t.id ASC
        LIMIT $5
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(project_ids)
    .bind(time_zone)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let mut metas = Vec::with_capacity(rows.len());
    let mut ids = Vec::with_capacity(rows.len());
    for row in &rows {
        let record = map_task_row(row)?;
        ids.push(record.id);
        let recurrence = row.try_get::<Option<Value>, _>("recurrence").ok().flatten();
        let due_key: Option<NaiveDate> = row.try_get("due_key")?;
        metas.push((row_to_meta(workspace_id, record, recurrence), due_key));
    }
    let (assignee_map, label_map) = load_task_refs(tx, workspace_id, &ids).await?;
    Ok(metas
        .into_iter()
        .map(|(meta, due_key)| {
            let id = meta.id;
            (
                TaskListItemRow {
                    assignee_ids: assignee_map.get(&id).cloned().unwrap_or_default(),
                    label_ids: label_map.get(&id).cloned().unwrap_or_default(),
                    meta,
                },
                due_key,
            )
        })
        .collect())
}

/// Import variant of [`create_task`] (source `importNotionDatabases` row
/// `importTx`): the creator must still be a workspace admin with edit access
/// to the project, the optional assignee is attached only while still a
/// member, and the task id joins the job's `created_refs` in the same
/// transaction. `Ok(Ok(None))` = the job's fence was lost.
#[allow(clippy::too_many_arguments)]
pub async fn create_import_task(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateTaskInput<'_>,
    assignee: Option<Uuid>,
    fence: crate::db::documents::ImportFence,
) -> Result<Result<Option<Uuid>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    let role = crate::db::workspace::membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|r| r.at_least(crate::db::workspace::WorkspaceRole::Admin)) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    let created = create_task_tx(
        &mut tx,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        input,
        None,
        "web",
    )
    .await?;
    let row = match created {
        Ok(row) => row,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if let Some(user_id) = assignee {
        sqlx::query(
            r#"
            INSERT INTO fvoci.task_assignees (workspace_id, task_id, user_id)
            SELECT $1, $2, $3
            WHERE EXISTS (
                SELECT 1 FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $3
            )
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(workspace_id)
        .bind(row.id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    if !crate::db::import_jobs::append_import_ref(
        &mut tx,
        workspace_id,
        fence.job_id,
        fence.lease_token,
        crate::db::import_jobs::ImportRefKind::Task,
        &row.id.to_string(),
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Ok(None));
    }
    tx.commit().await?;
    Ok(Ok(Some(row.id)))
}

/// Status ids and names of a project's workflow (import status matching).
pub async fn project_status_names(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, name FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows)
}

/// One logical CSV-row creation; the existing input and leased job are the
/// authority/identity. No task, ref or event commits independently.
pub(crate) struct ImportTaskRequest<'a> {
    pub claim: &'a ImportClaim,
    pub project_id: Uuid,
    pub input: CreateTaskInput<'a>,
    pub assignee: Option<Uuid>,
}

pub(crate) struct OriginTaskCreate<'a, 'input> {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub actor_user_id: Uuid,
    pub task_id: Uuid,
    pub input: &'a CreateTaskInput<'input>,
    pub client_ip: Option<&'a str>,
    pub channel: &'a str,
}

impl OperationTx<'_, '_> {
    /// Origin authorization/replay happens in the caller's same writer. No
    /// import claim, deferred event, autonomous transaction or parser is used.
    pub(crate) async fn create_origin_task_family(
        &mut self,
        request: OriginTaskCreate<'_, '_>,
    ) -> Result<Result<Uuid, ProjectDbError>, sqlx::Error> {
        let r = request;
        match self
            .insert_task_row_family(
                r.workspace_id,
                r.project_id,
                r.actor_user_id,
                r.input,
                r.task_id,
            )
            .await?
        {
            Ok(()) => {}
            Err(error) => return Ok(Err(error)),
        }
        let payload = json!({"taskId":r.task_id.to_string(),"projectId":r.project_id.to_string(),"title":r.input.title.trim()});
        self.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(r.workspace_id),
            actor_user_id: Some(r.actor_user_id),
            verb: "task.created".into(),
            target_type: Some("task".into()),
            target_id: Some(r.task_id),
            payload: payload.clone(),
        })
        .await?;
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(r.workspace_id),
            actor_user_id: Some(r.actor_user_id),
            verb: "task.created".into(),
            target_type: Some("task".into()),
            target_id: Some(r.task_id),
            payload,
            ip: r.client_ip.map(str::to_string),
        })
        .await?;
        self.record_created_task_activity_family(
            r.workspace_id,
            r.actor_user_id,
            r.task_id,
            r.channel,
        )
        .await?;
        Ok(Ok(r.task_id))
    }

    /// Selected counterpart of replace_task_assignees for the sole normal
    /// origin caller's self assignment, after the same-writer personal owner proof.
    pub(crate) async fn assign_origin_task_creator_family(
        &mut self,
        workspace: Uuid,
        project: Uuid,
        actor: Uuid,
        task: Uuid,
        client_ip: Option<&str>,
    ) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
        if self
            .membership_role(workspace, actor, false)
            .await?
            .is_none()
        {
            return Ok(Err(ProjectDbError::AssigneeIsNotAMember));
        }
        let Self::SqliteFamily(tx) = self else {
            unreachable!("family origin assignee")
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        let rows=tx.query("SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 ORDER BY user_id",&[Cell::uuid(workspace),Cell::uuid(task)]).await?;
        let current = rows
            .iter()
            .map(|row| row.cell(0)?.id())
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let changed = current.as_slice() != [actor];
        let added = if current.contains(&actor) {
            vec![]
        } else {
            vec![actor]
        };
        if !current.contains(&actor) {
            tx.execute("INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3) ON CONFLICT(task_id,user_id) DO NOTHING",&[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(actor)]).await?;
        }
        for user in &current {
            if *user != actor {
                tx.execute("DELETE FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 AND user_id=?3",&[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(*user)]).await?;
            }
        }
        if changed {
            let payload = json!({"taskId":task.to_string(),"projectId":project.to_string(),"assigneeIds":uuid_strings(&[actor]),"addedAssigneeIds":uuid_strings(&added)});
            self.append_event(EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: Some(actor),
                verb: "task.updated".into(),
                target_type: Some("task".into()),
                target_id: Some(task),
                payload: payload.clone(),
            })
            .await?;
            self.append_audit(AuditAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: Some(actor),
                verb: "task.updated".into(),
                target_type: Some("task".into()),
                target_id: Some(task),
                payload,
                ip: client_ip.map(str::to_string),
            })
            .await?;
        }
        Ok(Ok(()))
    }

    async fn import_task_authority(
        &mut self,
        claim: &ImportClaim,
        project: Uuid,
    ) -> Result<Result<bool, ProjectDbError>, sqlx::Error> {
        if self
            .require_import_admin(claim.workspace_id, claim.created_by, claim.session_id)
            .await?
            .is_err()
        {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if claim.source != crate::db::import_jobs::ImportSource::NotionZip
            || claim.project_id != Some(project)
            || !self.hold_import_claim(claim).await?
        {
            return Ok(Ok(false));
        }
        let Some((permission, archived)) = self
            .share_lock_project_permission(claim.workspace_id, claim.created_by, project)
            .await?
        else {
            return Ok(Err(ProjectDbError::NotFound));
        };
        if archived {
            return Ok(Err(ProjectDbError::Archived));
        }
        if !permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(ProjectDbError::NotFound));
        }
        Ok(Ok(true))
    }

    /// The caller owns current authority and commit. Import and normal origin
    /// creation share this exact hierarchy/status/number/sort/canonical row leaf.
    async fn insert_task_row_family(
        &mut self,
        workspace: Uuid,
        project: Uuid,
        actor: Uuid,
        input: &CreateTaskInput<'_>,
        task_id: Uuid,
    ) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            unreachable!("family task insertion")
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        if input.task_type == "subtask" && input.parent_id.is_none() {
            return Ok(Err(ProjectDbError::Conflict));
        }
        if let Some(parent) = input.parent_id {
            let rows = tx
                .query(
                    "SELECT project_id,deleted_at,type FROM tasks WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(parent)],
                )
                .await?;
            let Some(row) = rows.first() else {
                return Ok(Err(ProjectDbError::NotFound));
            };
            if row.cell(0)?.id()? != project || row.cell(1)?.optional(Cell::integer)?.is_some() {
                return Ok(Err(ProjectDbError::NotFound));
            }
            if violates_task_hierarchy(input.task_type, &row.cell(2)?.string()?) {
                return Ok(Err(ProjectDbError::Conflict));
            }
        }
        if let Some(milestone) = input.milestone_id {
            if tx
                .query(
                    "SELECT id FROM milestones WHERE workspace_id=?1 AND project_id=?2 AND id=?3",
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(project),
                        Cell::uuid(milestone),
                    ],
                )
                .await?
                .is_empty()
            {
                return Ok(Err(ProjectDbError::MilestoneNotFound));
            }
        }
        let statuses=tx.query("SELECT id,category FROM statuses WHERE workspace_id=?1 AND project_id=?2 ORDER BY sort_key COLLATE BINARY", &[Cell::uuid(workspace),Cell::uuid(project)]).await?;
        let status = if let Some(status) = input.status_id {
            let mut valid = false;
            for row in &statuses {
                if row.cell(0)?.id()? == status {
                    valid = true;
                }
            }
            if !valid {
                return Ok(Err(ProjectDbError::StatusNotInWorkflow));
            }
            status
        } else {
            let mut fallback = None;
            for row in &statuses {
                let id = row.cell(0)?.id()?;
                fallback.get_or_insert(id);
                if row.cell(1)?.string()? == "backlog" {
                    fallback = Some(id);
                    break;
                }
            }
            let Some(status) = fallback else {
                return Ok(Err(ProjectDbError::NotFound));
            };
            status
        };
        let numbers=tx.query("UPDATE projects SET next_number=next_number+1,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND id=?2 RETURNING next_number-1", &[Cell::uuid(workspace),Cell::uuid(project)]).await?;
        let number = numbers
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .int32()?;
        let last=tx.query("SELECT sort_key FROM tasks WHERE workspace_id=?1 AND project_id=?2 AND status_id=?3 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY DESC LIMIT 1", &[Cell::uuid(workspace),Cell::uuid(project),Cell::uuid(status)]).await?;
        let last = last.first().map(|r| r.cell(0)?.string()).transpose()?;
        let sort = match between(last.as_deref(), None) {
            Ok(sort) => sort,
            Err(_) => return Ok(Err(ProjectDbError::Conflict)),
        };
        let start = input.start_date.map(|d| d.to_string());
        let due = input.due_date.map(|d| d.to_string());
        tx.execute("INSERT INTO tasks(id,workspace_id,project_id,number,title,type,priority,status_id,start_date,due_date,parent_id,milestone_id,recurrence,sort_key,schema_version,content_json,created_by) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)", &[
            Cell::uuid(task_id),Cell::uuid(workspace),Cell::uuid(project),Cell::Integer(i64::from(number)),Cell::text(input.title.trim()),Cell::text(input.task_type),Cell::text(input.priority),Cell::uuid(status),Cell::optional_text(start.as_deref()),Cell::optional_text(due.as_deref()),Cell::optional_uuid(input.parent_id),Cell::optional_uuid(input.milestone_id),input.recurrence.as_ref().map(Cell::json).transpose()?.unwrap_or(Cell::Null),Cell::text(sort),Cell::Integer(i64::from(DOCUMENT_SCHEMA_VERSION)),Cell::json(&empty_document_json())?,Cell::uuid(actor)
        ]).await?;
        Ok(Ok(()))
    }

    async fn record_created_task_activity_family(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        task_id: Uuid,
        channel: &str,
    ) -> Result<(), sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            unreachable!("family task activity")
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        // The existing create activity uses an empty snapshot: diff_activity
        // produces this canonical empty created change, independently of CSV data.
        let changes = crate::tasks::activity::diff_activity(None, &ActivitySnapshot::new())
            .map(Value::Array)
            .unwrap_or_else(|| json!([]));
        tx.execute("INSERT INTO task_activity(id,workspace_id,task_id,actor_user_id,channel,kind,changes) VALUES(?1,?2,?3,?4,?5,'created',?6)", &[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(task_id),Cell::uuid(actor),Cell::text(channel),Cell::json(&changes)?]).await?;
        Ok(())
    }

    /// Family leaf of the existing task creation program: reuse current
    /// authorization, hierarchy, sort allocation, schema and event/audit APIs.
    async fn create_import_task_family(
        &mut self,
        request: &ImportTaskRequest<'_>,
        task_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Result<Option<Uuid>, ProjectDbError>, sqlx::Error> {
        let c = request.claim;
        let workspace = c.workspace_id;
        let project = request.project_id;
        let input = &request.input;
        match self.import_task_authority(c, project).await? {
            Err(error) => return Ok(Err(error)),
            Ok(false) => return Ok(Ok(None)),
            Ok(true) => {}
        }
        if cancel.is_cancelled() {
            return Ok(Ok(None));
        }
        match self
            .insert_task_row_family(workspace, project, c.created_by, input, task_id)
            .await?
        {
            Ok(()) => {}
            Err(error) => return Ok(Err(error)),
        }
        let Self::SqliteFamily(tx) = self else {
            unreachable!("family import assignment")
        };
        if let Some(assignee) = request.assignee {
            tx.execute("INSERT INTO task_assignees(workspace_id,task_id,user_id) SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM memberships WHERE workspace_id=?1 AND user_id=?3) ON CONFLICT DO NOTHING", &[Cell::uuid(workspace),Cell::uuid(task_id),Cell::uuid(assignee)]).await?;
        }
        self.record_created_task_activity_family(workspace, c.created_by, task_id, "web")
            .await?;
        let fence = crate::db::documents::ImportFence {
            job_id: c.job_id,
            lease_token: c.lease_token,
        };
        let payload = json!({"taskId":task_id.to_string(),"projectId":project.to_string(),"title":input.title.trim()});
        if !self
            .park_import_event(
                workspace,
                fence,
                EventAppend {
                    id: Uuid::now_v7(),
                    workspace_id: Some(workspace),
                    actor_user_id: Some(c.created_by),
                    verb: "task.created".into(),
                    target_type: Some("task".into()),
                    target_id: Some(task_id),
                    payload: payload.clone(),
                },
                "web",
            )
            .await?
        {
            return Ok(Ok(None));
        }
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(c.created_by),
            verb: "task.created".into(),
            target_type: Some("task".into()),
            target_id: Some(task_id),
            payload,
            ip: None,
        })
        .await?;
        if !self
            .append_import_ref(
                workspace,
                fence,
                crate::db::import_jobs::ImportRefKind::Task,
                &task_id.to_string(),
            )
            .await?
        {
            return Ok(Ok(None));
        }
        match self.import_task_authority(c, project).await? {
            Ok(true) if !cancel.is_cancelled() => Ok(Ok(Some(task_id))),
            Ok(_) => Ok(Ok(None)),
            Err(error) => Ok(Err(error)),
        }
    }
}

#[cfg(test)]
tokio::task_local! {
    static IMPORT_TASK_ROLLBACK_AFTER_ACK_CONTROL: bool;
}

#[derive(Debug, thiserror::Error)]
enum ImportTaskRollbackReason {
    #[error("import task refused: {0:?}")]
    Domain(ProjectDbError),
    #[error("import task publication fenced or cancelled")]
    FencedOrCancelled,
}

pub(crate) async fn create_import_task_backend(
    backend: &Backend,
    request: ImportTaskRequest<'_>,
    cancel: &CancellationToken,
) -> Result<Result<Option<Uuid>, ProjectDbError>, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(Ok(None));
    }
    if let Backend::Postgres(pool) = backend {
        let c = request.claim;
        return create_import_task(
            pool,
            c.workspace_id,
            request.project_id,
            c.created_by,
            c.session_id,
            request.input,
            request.assignee,
            crate::db::documents::ImportFence {
                job_id: c.job_id,
                lease_token: c.lease_token,
            },
        )
        .await;
    }
    // Choose once per logical row, before writer acquisition. Unknown remote
    // commit retains the original error and never retries with a fresh UUID.
    let task_id = Uuid::now_v7();
    let mut tx = backend.begin_write().await?;
    tx.operation()
        .set_tenant(request.claim.workspace_id)
        .await?;
    let result = tx
        .operation()
        .create_import_task_family(&request, task_id, cancel)
        .await;
    match result {
        Ok(Ok(Some(id))) if !cancel.is_cancelled() => {
            tx.commit()
                .await
                .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
            Ok(Ok(Some(id)))
        }
        Ok(Ok(_)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(ImportTaskRollbackReason::FencedOrCancelled)),
                    cleanup,
                ));
            }
            Ok(Ok(None))
        }
        Ok(Err(error)) => {
            let cleanup = tx.rollback().await;
            // Test propagation only, after an actual acknowledged local
            // rollback; this is not provider settlement evidence.
            #[cfg(test)]
            let cleanup = cleanup.and_then(|()| {
                if IMPORT_TASK_ROLLBACK_AFTER_ACK_CONTROL
                    .try_with(|enabled| *enabled)
                    .unwrap_or(false)
                {
                    Err(sqlx::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "after-real-task-rollback propagation control",
                    )))
                } else {
                    Ok(())
                }
            });
            if let Err(cleanup) = cleanup {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(ImportTaskRollbackReason::Domain(error))),
                    cleanup,
                ));
            }
            Ok(Err(error))
        }
        Err(error) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

pub(crate) async fn project_status_names_backend(
    backend: &Backend,
    claim: &ImportClaim,
    project: Uuid,
) -> Result<Result<Vec<(Uuid, String)>, ProjectDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return project_status_names(pool, claim.workspace_id, project)
            .await
            .map(Ok);
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(claim.workspace_id).await?;
    let result=async {
        match tx.operation().import_task_authority(claim,project).await? {
            Ok(true)=>{}, Ok(false)=>return Ok(Err(ProjectDbError::NotFound)),Err(e)=>return Ok(Err(e))
        }
        let mut op=tx.operation();
        let OperationTx::SqliteFamily(family)=&mut op else{unreachable!("family read")};
        family.query("SELECT id,name FROM statuses WHERE workspace_id=?1 AND project_id=?2 ORDER BY sort_key COLLATE BINARY,id", &[Cell::uuid(claim.workspace_id),Cell::uuid(project)]).await?.iter().map(|r|Ok((r.cell(0)?.id()?,r.cell(1)?.string()?))).collect::<Result<Vec<_>,sqlx::Error>>().map(Ok)
    }.await;
    // Status matching is observational; the borrowed authority check's lease
    // refresh is deliberately rolled back, and no writer result is fabricated.
    if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Err(error) => Some(Box::new(error)),
            Ok(Err(error)) => Some(Box::new(ImportTaskRollbackReason::Domain(error))),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}

#[cfg(test)]
mod selected_import_task_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::import_jobs::{
        claim_next_import_job_backend, create_async_import_job_backend, ImportSource,
        NewAsyncImport,
    };

    async fn setup() -> (Fixture, ImportClaim, Uuid, Uuid) {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                &crate::auth::token::new_token().hash,
                DateTime::from_timestamp_micros(Utc::now().timestamp_micros() + 86_400_000_000)
                    .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let project = Uuid::now_v7();
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'IMP','Import','private',?3)").bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        // Normal private-project creation installs the creator's lead grant;
        // workspace admin status alone does not grant private-project access.
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')")
            .bind(Uuid::now_v7().as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'할 일','backlog','V')").bind(status.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        create_async_import_job_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            ImportSource::NotionZip,
            NewAsyncImport {
                file_name: Some("actual-notion.zip"),
                project_id: Some(project),
                payload: b"source preserved",
            },
        )
        .await
        .unwrap()
        .unwrap();
        let claim = claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        (f, claim, project, status)
    }
    fn request(
        claim: &ImportClaim,
        project: Uuid,
        status: Option<Uuid>,
        assignee: Option<Uuid>,
    ) -> ImportTaskRequest<'_> {
        ImportTaskRequest {
            claim,
            project_id: project,
            input: CreateTaskInput {
                title: "  실제 CSV 작업 😀  ",
                task_type: "task",
                priority: "none",
                status_id: status,
                start_date: None,
                due_date: NaiveDate::from_ymd_opt(2026, 10, 5),
                parent_id: None,
                milestone_id: None,
                recurrence: None,
            },
            assignee,
        }
    }
    async fn no_effects(f: &Fixture) {
        let counts:(i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM tasks),(SELECT next_number FROM projects),(SELECT count(*) FROM import_deferred_events),(SELECT count(*) FROM task_activity)").fetch_one(&f.pool).await.unwrap();
        assert_eq!(counts, (0, 1, 0, 0));
    }
    #[tokio::test]
    async fn import_selected_task_private_project_current_grant_required_then_healthy() {
        let (f, c, project, status) = setup().await;
        sqlx::query(
            "DELETE FROM project_members WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(project.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(matches!(
            create_import_task_backend(
                &f.backend,
                request(&c, project, Some(status), None),
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        no_effects(&f).await;
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .execute(&f.pool).await.unwrap();
        let id = create_import_task_backend(
            &f.backend,
            request(&c, project, Some(status), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let refs: String = sqlx::query_scalar("SELECT created_refs FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::db::import_jobs::ImportJobRefs>(&refs)
                .unwrap()
                .task_ids,
            vec![id]
        );
        let row: (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM tasks),(SELECT next_number FROM projects),(SELECT count(*) FROM import_deferred_events)")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(row, (1, 2, 1));
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_task_rollback_control_retains_domain_and_healthy_retry() {
        let (f, c, project, status) = setup().await;
        let missing_status = Uuid::now_v7();
        let error = IMPORT_TASK_ROLLBACK_AFTER_ACK_CONTROL
            .scope(
                true,
                create_import_task_backend(
                    &f.backend,
                    request(&c, project, Some(missing_status), None),
                    &CancellationToken::new(),
                ),
            )
            .await
            .unwrap_err();
        assert!(crate::import_job::import_database_error_stops_scheduler(
            &f.backend, &error
        ));
        let sqlx::Error::AnyDriverError(source) = &error else {
            panic!("typed rollback error required");
        };
        let stopped = source
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        let original = stopped
            .original
            .as_ref()
            .unwrap()
            .downcast_ref::<ImportTaskRollbackReason>()
            .unwrap();
        assert!(matches!(
            original,
            ImportTaskRollbackReason::Domain(ProjectDbError::StatusNotInWorkflow)
        ));
        assert!(
            matches!(&stopped.cleanup, sqlx::Error::Io(error) if error.kind()==std::io::ErrorKind::ConnectionAborted)
        );
        no_effects(&f).await;
        let refs: String = sqlx::query_scalar("SELECT created_refs FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert!(
            serde_json::from_str::<crate::db::import_jobs::ImportJobRefs>(&refs)
                .unwrap()
                .is_empty()
        );
        // A confirmed rollback still returns the exact existing domain refusal.
        assert!(matches!(
            create_import_task_backend(
                &f.backend,
                request(&c, project, Some(missing_status), None),
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            Err(ProjectDbError::StatusNotInWorkflow)
        ));
        no_effects(&f).await;
        let id = create_import_task_backend(
            &f.backend,
            request(&c, project, Some(status), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let row: (String, Vec<u8>) =
            sqlx::query_as("SELECT title,status_id FROM tasks WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(row, ("실제 CSV 작업 😀".into(), status.as_bytes().to_vec()));
        let refs: String = sqlx::query_scalar("SELECT created_refs FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<crate::db::import_jobs::ImportJobRefs>(&refs)
                .unwrap()
                .task_ids,
            vec![id]
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_task_literal_dates_refs_activity_deferred_event_and_current_member() {
        let (f, c, project, status) = setup().await;
        assert_eq!(
            project_status_names_backend(&f.backend, &c, project)
                .await
                .unwrap()
                .unwrap(),
            vec![(status, "할 일".into())]
        );
        let id = create_import_task_backend(
            &f.backend,
            request(&c, project, None, Some(f.user)),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(id.get_version_num(), 7);
        let row:(String,String,i64,Vec<u8>,String,i64,i64)=sqlx::query_as("SELECT title,due_date,number,status_id,content_json,schema_version,version FROM tasks WHERE id=?1").bind(id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            row,
            (
                "실제 CSV 작업 😀".into(),
                "2026-10-05".into(),
                1,
                status.as_bytes().to_vec(),
                serde_json::to_string(&empty_document_json()).unwrap(),
                i64::from(DOCUMENT_SCHEMA_VERSION),
                1
            )
        );
        let refs: String = sqlx::query_scalar("SELECT created_refs FROM import_jobs WHERE id=?1")
            .bind(c.job_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let refs: crate::db::import_jobs::ImportJobRefs = serde_json::from_str(&refs).unwrap();
        assert_eq!(refs.task_ids, vec![id]);
        let counts:(i64,i64,i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM task_assignees),(SELECT count(*) FROM task_activity WHERE kind='created' AND channel='web' AND changes='[]'),(SELECT count(*) FROM import_deferred_events WHERE verb='task.created'),(SELECT count(*) FROM events),(SELECT next_number FROM projects)").fetch_one(&f.pool).await.unwrap();
        assert_eq!(counts, (1, 1, 1, 0, 2));
        let second = create_import_task_backend(
            &f.backend,
            request(&c, project, Some(status), Some(Uuid::now_v7())),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let row: (i64, String) = sqlx::query_as("SELECT number,sort_key FROM tasks WHERE id=?1")
            .bind(second.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(row.0, 2);
        let first_sort: String = sqlx::query_scalar("SELECT sort_key FROM tasks WHERE id=?1")
            .bind(id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert!(row.1 > first_sort);
        let assigned: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_assignees WHERE task_id=?1")
                .bind(second.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(assigned, 0);
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(&f.root).unwrap();
    }
    #[tokio::test]
    async fn import_selected_task_wrong_status_owner_tenant_cancel_and_revoked_session_no_effects()
    {
        let (f, c, project, _) = setup().await;
        assert!(matches!(
            create_import_task_backend(
                &f.backend,
                request(&c, project, Some(Uuid::now_v7()), None),
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            Err(ProjectDbError::StatusNotInWorkflow)
        ));
        no_effects(&f).await;
        let mut forged = c.clone();
        forged.lease_token = Uuid::now_v7();
        assert!(create_import_task_backend(
            &f.backend,
            request(&forged, project, None, None),
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .unwrap()
        .is_none());
        no_effects(&f).await;
        forged = c.clone();
        forged.workspace_id = Uuid::now_v7();
        assert!(create_import_task_backend(
            &f.backend,
            request(&forged, project, None, None),
            &CancellationToken::new()
        )
        .await
        .unwrap()
        .is_err());
        no_effects(&f).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(
            create_import_task_backend(&f.backend, request(&c, project, None, None), &cancel)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        no_effects(&f).await;
        sqlx::query("DELETE FROM sessions WHERE id=?1")
            .bind(c.session_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_import_task_backend(
                &f.backend,
                request(&c, project, None, None),
                &CancellationToken::new()
            )
            .await
            .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        no_effects(&f).await;
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

#[cfg(test)]
mod selected_task_projection_tests {
    use super::*;

    fn projection(id: u128) -> TaskProjection {
        TaskProjection {
            id: Uuid::from_u128(id),
            number: id as i32,
            title: "literal %_\\ 😀".into(),
            priority: "none".into(),
            status: Uuid::nil(),
            start: None,
            due_date: None,
            due_at: None,
            rank: "V".into(),
            created: DateTime::from_timestamp(1760000000, 0).unwrap(),
            updated: DateTime::from_timestamp(1760000000, 0).unwrap(),
            status_rank: "V".into(),
            scalars: HashMap::new(),
        }
    }

    #[test]
    fn selected_decimal_orders_exact_large_and_fractional_values() {
        let values = [
            "9007199254740993",
            "10",
            "9",
            "9007199254740992",
            "0.0000000000000000002",
            "0.0000000000000000001",
            "-10",
        ];
        let mut sorted = values
            .into_iter()
            .map(|value| (decimal_value(value).unwrap(), value))
            .collect::<Vec<_>>();
        sorted.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            sorted.iter().map(|entry| entry.1).collect::<Vec<_>>(),
            vec![
                "-10",
                "0.0000000000000000001",
                "0.0000000000000000002",
                "9",
                "10",
                "9007199254740992",
                "9007199254740993"
            ]
        );
        assert_eq!(
            decimal_value("+009.000").unwrap(),
            decimal_value("9").unwrap()
        );
        assert_ne!(
            decimal_value("9007199254740992").unwrap(),
            decimal_value("9007199254740993").unwrap()
        );
    }

    #[test]
    fn selected_query_fraction_matches_pg7_text_cast_vectors() {
        let locale = TaskTextLocale::new().unwrap();
        // Original restricted PG18.3 observation P01-P18, including negative
        // and PG-epoch boundaries: half-up and epoch truncation both fail.
        for base in [
            "1969-12-31T23:59:59",
            "1999-12-31T23:59:59",
            "2026-12-31T23:59:59",
        ] {
            for (fraction, micros) in [
                ("000000499", 0u32),
                ("000000500", 0),
                ("000000501", 1),
                ("999999499", 999999),
                ("999999500", 1_000_000),
                ("999999501", 1_000_000),
            ] {
                let at = crate::tasks::parse_iso_datetime(&format!("{base}.{fraction}Z")).unwrap();
                let actual = task_query_instant(at).unwrap();
                assert_eq!(
                    actual.timestamp(),
                    at.timestamp() + i64::from(micros / 1_000_000)
                );
                assert_eq!(actual.timestamp_subsec_nanos(), (micros % 1_000_000) * 1000);
            }
        }
        let input = crate::tasks::parse_iso_datetime("9999-12-31T23:59:59.999999501Z").unwrap();
        assert_eq!(
            task_query_instant(input).unwrap().to_rfc3339(),
            "+10000-01-01T00:00:00+00:00"
        );
        let literal = crate::tasks::parse_iso_datetime("2026-01-01T00:00:00.000000500Z").unwrap();
        assert_eq!(
            TaskScalar::from_policy(&ScalarPolicy::Instant(literal)).unwrap(),
            TaskScalar::Instant(crate::tasks::parse_iso_datetime("2026-01-01T00:00:00Z").unwrap())
        );
        let mut query =
            crate::tasks::list_query::parse_task_list_query(None, None, None, Some(50), None, None)
                .unwrap();
        query.as_of = literal;
        let mut row = projection(1);
        row.created = crate::tasks::parse_iso_datetime("2026-01-01T00:00:00.000001Z").unwrap();
        assert!(!task_projection_matches(
            &row,
            &query,
            &TaskTimeZone::from_name("UTC").unwrap(),
            &locale
        )
        .unwrap());
    }

    #[test]
    fn selected_top_k_nulls_last_both_directions_and_uuid_ties() {
        let locale = TaskTextLocale::new().unwrap();
        let field = Uuid::from_u128(999);
        let zone = TaskTimeZone::from_name("UTC").unwrap();
        for (direction, expected) in [
            (SortDirection::Asc, vec![1, 2, 3, 4]),
            (SortDirection::Desc, vec![3, 1, 2, 4]),
        ] {
            let sort = vec![ViewSort {
                field: SortField::Field(field),
                direction,
            }];
            let mut selected = Vec::new();
            for (id, number) in [
                (5, None),
                (3, Some("10")),
                (2, Some("9")),
                (4, None),
                (1, Some("9")),
            ] {
                let mut row = projection(id);
                if let Some(number) = number {
                    row.scalars
                        .insert(field, TaskScalar::Number(decimal_value(number).unwrap()));
                }
                retain_task_top_k(
                    &mut selected,
                    TaskPageEntry {
                        key: row.page_key(&sort, &zone).unwrap(),
                        cursor_key: row.cursor_key(&sort, &zone).unwrap(),
                    },
                    4,
                    &sort,
                    &locale,
                )
                .unwrap();
                assert!(selected.len() <= 4);
            }
            assert_eq!(
                selected
                    .iter()
                    .map(|entry| entry.key.id.as_u128())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn selected_tzif_preserves_year_9999_dst_and_due_date_precedence() {
        let utc = TaskTimeZone::from_name("UTC").unwrap();
        let ny = TaskTimeZone::from_name("America/New_York").unwrap();
        let parsed = |raw| crate::tasks::parse_iso_datetime(raw).unwrap();
        assert_eq!(
            utc.date(parsed("9999-12-31T23:59:59.999999Z"))
                .unwrap()
                .to_string(),
            "9999-12-31"
        );
        for (raw, date) in [
            ("2026-03-08T04:59:59Z", "2026-03-07"),
            ("2026-03-08T05:00:00Z", "2026-03-08"),
            ("2026-11-01T05:30:00Z", "2026-11-01"),
            ("2026-11-01T06:30:00Z", "2026-11-01"),
            ("2699-07-01T03:30:00Z", "2699-06-30"),
        ] {
            assert_eq!(ny.date(parsed(raw)).unwrap().to_string(), date);
        }
        let mut row = projection(1);
        row.due_at = Some(parsed("2026-03-08T04:59:59Z"));
        row.due_date = Some(NaiveDate::from_ymd_opt(2026, 3, 10).unwrap());
        assert_eq!(row.due(&ny).unwrap(), row.due_date);
        for unknown in ["america/new_york", "Mars/Olympus", " UTC "] {
            assert_eq!(TaskTimeZone::from_name(unknown).unwrap().name, "UTC");
        }
    }

    #[test]
    fn selected_window_uses_utc_endpoints_and_literal_title_characters() {
        let locale = TaskTextLocale::new().unwrap();
        let mut query = crate::tasks::list_query::parse_task_list_query(
            None,
            None,
            None,
            Some(50),
            Some("2026-03-08"),
            Some("2026-03-08"),
        )
        .unwrap();
        query.as_of = DateTime::from_timestamp(2000000000, 0).unwrap();
        query.view.filters.title = Some("%_\\".into());
        let mut row = projection(1);
        row.due_at = Some(crate::tasks::parse_iso_datetime("2026-03-08T04:30:00Z").unwrap());
        let zone = TaskTimeZone::from_name("America/New_York").unwrap();
        assert_eq!(row.due(&zone).unwrap().unwrap().to_string(), "2026-03-07");
        assert!(task_projection_matches(&row, &query, &zone, &locale).unwrap());
        row.title = "arbitrary wildcard match".into();
        assert!(!task_projection_matches(&row, &query, &zone, &locale).unwrap());
        row.title = "literal %_\\ 😀".into();
        row.due_at = None;
        assert!(!task_projection_matches(&row, &query, &zone, &locale).unwrap());
        row.start = Some(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap());
        assert!(task_projection_matches(&row, &query, &zone, &locale).unwrap());
    }

    #[test]
    fn selected_title_byte_order_and_cursor_changed_value_refusal() {
        let locale = TaskTextLocale::new().unwrap();
        let zone = TaskTimeZone::from_name("UTC").unwrap();
        let sort = vec![ViewSort {
            field: SortField::Title,
            direction: SortDirection::Asc,
        }];
        let mut rows = ["😀", "é", "e\u{301}", "中", "Z", "a"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| {
                let mut row = projection(index as u128 + 1);
                row.title = title.into();
                row
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            compare_task_page_key(
                &left.page_key(&sort, &zone).unwrap(),
                &right.page_key(&sort, &zone).unwrap(),
                &sort,
                &locale,
            )
            .unwrap()
        });
        assert_eq!(
            rows.iter()
                .map(|row| row.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Z", "a", "e\u{301}", "é", "中", "😀"]
        );
        let key = rows[0].cursor_key(&sort, &zone).unwrap();
        rows[0].title.push('!');
        assert_ne!(key, rows[0].cursor_key(&sort, &zone).unwrap());
    }
    #[test]
    fn selected_gnu_literal_lower_matches_original_pg_profile() {
        // Actual sealed PG18.3 S8/S9 rows, not expectations from this wrapper.
        let locale = TaskTextLocale::new().unwrap();
        for (input, expected_hex) in [
            ("Σ", "cf83"),
            ("σ", "cf83"),
            ("ς", "cf82"),
            ("ΟΔΥΣΣΕΥΣ", "cebfceb4cf85cf83cf83ceb5cf85cf83"),
            ("Οδυσσεύς", "cebfceb4cf85cf83cf83ceb5cf8dcf82"),
            ("σς", "cf83cf82"),
            ("ΣΑΣ", "cf83ceb1cf83"),
            ("σας", "cf83ceb1cf82"),
            ("İ", "69"),
            ("ı", "c4b1"),
            ("I", "69"),
            ("i", "69"),
            ("İstanbul", "697374616e62756c"),
            ("ISTANBUL", "697374616e62756c"),
            ("istanbul", "697374616e62756c"),
            ("ıi", "c4b169"),
            ("ß", "c39f"),
            ("ẞ", "c39f"),
            ("Straße", "73747261c39f65"),
            ("STRASSE", "73747261737365"),
            ("ﬁ", "efac81"),
            ("ǅ", "c786"),
            ("Ǆ", "c786"),
            ("ǆ", "c786"),
        ] {
            assert_eq!(
                hex::encode(locale.lower_literal(input).unwrap()),
                expected_hex
            );
        }
        for (left, right, equal) in [
            ("İ", "i", true),
            ("İ", "I", true),
            ("ı", "I", false),
            ("ı", "i", false),
            ("İstanbul", "istanbul", true),
            ("İstanbul", "ISTANBUL", true),
            ("Σ", "σ", true),
            ("Σ", "ς", false),
            ("σ", "ς", false),
            ("ΟΔΥΣΣΕΥΣ", "Οδυσσεύς", false),
            ("ΟΔΥΣΣΕΥΣ", "οδυσσευς", false),
            ("ß", "ss", false),
            ("ß", "ẞ", true),
            ("Straße", "STRASSE", false),
            ("ǅ", "ǆ", true),
            ("ǅ", "Ǆ", true),
        ] {
            assert_eq!(
                locale.lower_literal(left).unwrap() == locale.lower_literal(right).unwrap(),
                equal
            );
        }
        assert!(locale
            .lower_literal("literal %_\\ 😀")
            .unwrap()
            .contains("%_\\"));
        assert_ne!(
            locale.lower_literal("é").unwrap(),
            locale.lower_literal("e\u{301}").unwrap()
        );
        assert!(locale.lower_literal("bad\0text").is_err());
    }

    #[test]
    fn selected_gnu_default_and_c_sort_use_original_pg_orders() {
        let locale = TaskTextLocale::new().unwrap();
        let zone = TaskTimeZone::from_name("UTC").unwrap();
        let field = Uuid::from_u128(123);
        let default = [
            "e", "E", "e\u{301}", "é", "i", "I", "İ", "ı", "ß", "ẞ", "z", "Z", "σ", "Σ", "ς", "中",
        ];
        let c = [
            "E", "I", "Z", "e", "e\u{301}", "i", "z", "ß", "é", "İ", "ı", "Σ", "ς", "σ", "ẞ", "中",
        ];
        for (sort_field, expected) in [
            (SortField::Title, c),
            (SortField::Rank, c),
            (SortField::Status, default),
            (SortField::Field(field), default),
        ] {
            for direction in [SortDirection::Asc, SortDirection::Desc] {
                let sort = [ViewSort {
                    field: sort_field,
                    direction,
                }];
                let mut selected = Vec::new();
                for (index, text) in c.iter().enumerate() {
                    let mut row = projection(index as u128 + 1);
                    row.title = (*text).into();
                    row.rank = (*text).into();
                    row.status_rank = (*text).into();
                    row.scalars.insert(field, TaskScalar::Text((*text).into()));
                    retain_task_top_k(
                        &mut selected,
                        TaskPageEntry {
                            key: row.page_key(&sort, &zone).unwrap(),
                            cursor_key: (*text).into(),
                        },
                        16,
                        &sort,
                        &locale,
                    )
                    .unwrap();
                }
                let mut expected = expected.to_vec();
                if direction == SortDirection::Desc {
                    expected.reverse();
                }
                assert_eq!(
                    selected
                        .iter()
                        .map(|row| row.cursor_key.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
                let anchor = selected[7].key.clone();
                assert_eq!(
                    selected
                        .iter()
                        .filter(
                            |row| compare_task_page_key(&row.key, &anchor, &sort, &locale).unwrap()
                                == Ordering::Greater
                        )
                        .count(),
                    8
                );
            }
        }
        // Actual PG keeps canonically equivalent spellings byte-distinct.
        assert_eq!(locale.compare("e\u{301}", "é").unwrap(), Ordering::Less);
    }

    #[test]
    fn selected_gnu_wrong_plan_and_nul_preserve_selection() {
        assert!(TaskTextLocale::open(c"FVOCI_invalid_qualification_locale").is_err());
        let locale = TaskTextLocale::new().unwrap();
        let zone = TaskTimeZone::from_name("UTC").unwrap();
        let sort = [ViewSort {
            field: SortField::Status,
            direction: SortDirection::Asc,
        }];
        let row = projection(1);
        let mut selected = Vec::new();
        retain_task_top_k(
            &mut selected,
            TaskPageEntry {
                key: row.page_key(&sort, &zone).unwrap(),
                cursor_key: "healthy".into(),
            },
            2,
            &sort,
            &locale,
        )
        .unwrap();
        for bad in [
            TaskPageKey {
                cells: vec![],
                id: Uuid::from_u128(2),
            },
            TaskPageKey {
                cells: vec![TaskOrderCell {
                    value: Some(TaskScalar::Text("bad\0text".into())),
                    desc: false,
                }],
                id: Uuid::from_u128(2),
            },
            TaskPageKey {
                cells: vec![TaskOrderCell {
                    value: Some(TaskScalar::Text("V".into())),
                    desc: true,
                }],
                id: Uuid::from_u128(2),
            },
        ] {
            assert!(retain_task_top_k(
                &mut selected,
                TaskPageEntry {
                    key: bad,
                    cursor_key: "bad".into()
                },
                2,
                &sort,
                &locale
            )
            .is_err());
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].cursor_key, "healthy");
        }
        let wrong_type = TaskPageKey {
            cells: vec![TaskOrderCell {
                value: Some(TaskScalar::Integer(1)),
                desc: false,
            }],
            id: Uuid::from_u128(2),
        };
        assert!(compare_task_page_key(&selected[0].key, &wrong_type, &sort, &locale).is_err());
        assert!(locale.compare("bad\0text", "valid").is_err());
        assert_eq!(locale.compare("valid", "valid").unwrap(), Ordering::Equal);
    }

    #[test]
    fn selected_scalar_profile_rejects_wrong_catalog_hash_schema_and_unqualified_runtime() {
        use base64::Engine;
        let raw = include_str!("task_scalar_pg18_profile.json");
        let original: Value = serde_json::from_str(raw).unwrap();
        let profile = TaskScalarProfile::decode(raw).unwrap();
        assert_eq!(profile.zones.len(), 487);
        assert_eq!(
            profile.data_identity,
            "87c748703be14b07e96a29ee04e031bca7c2f1f60267c9760af76b8de40bad71"
        );
        for name in ["UTC", "localtime", "posixrules"] {
            assert!(profile.zones.iter().any(|zone| zone.name == name));
        }
        for bad in [
            {
                let mut value = original.clone();
                value["reference"]["provider"] = json!("i");
                value
            },
            {
                let mut value = original.clone();
                value["unexpected"] = json!(true);
                value
            },
            {
                let mut value = original.clone();
                value["zones"][0]["name"] = json!("Mars/Olympus");
                value
            },
            {
                let mut value = original.clone();
                value["zones"][0]["sha256"] = json!("0".repeat(64));
                value
            },
            {
                let mut value = original.clone();
                let data = b"invalid TZif";
                value["zones"][0]["tzif_base64"] =
                    json!(base64::engine::general_purpose::STANDARD.encode(data));
                value["zones"][0]["sha256"] = json!(task_scalar_sha256(data));
                value
            },
        ] {
            assert!(TaskScalarProfile::decode(&bad.to_string()).is_err());
        }
        let mut pending = original.clone();
        pending["runtime_profiles"] = json!([]);
        let pending = TaskScalarProfile::decode(&pending.to_string()).unwrap();
        assert!(TaskTextLocale::for_profile(&pending).is_err());
        // No fabricated architecture qualification is supplied by the test.
        assert!(pending.runtime_profiles.is_empty());
        let same_data = TaskScalarProfile::decode(&format!("{raw}\n")).unwrap();
        assert_eq!(same_data.data_identity, profile.data_identity);
        assert_ne!(same_data.identity, profile.identity);
        assert_eq!(
            TaskScalarProfile::decode(raw).unwrap().identity,
            profile.identity
        );
        assert!(matches!(
            TaskScalarProfile::decode_sealed(
                &format!("{raw}\n"),
                &task_scalar_sha256(raw.as_bytes())
            ),
            Err("Task scalar compiled profile checksum mismatch")
        ));
    }

    #[test]
    fn selected_scalar_full_catalog_uses_reference_data_and_exact_case_fallback() {
        let profile = TaskScalarProfile::compiled().unwrap();
        assert_eq!(profile.zones.len(), 487);
        for zone in &profile.zones {
            let actual = TaskTimeZone::from_name(&zone.name).unwrap();
            assert_eq!(actual.name, zone.name);
            actual
                .date(crate::tasks::parse_iso_datetime("9999-12-31T12:00:00Z").unwrap())
                .unwrap();
        }
        for unknown in ["america/new_york", "Mars/Olympus", "US/Eastern", " UTC "] {
            assert_eq!(TaskTimeZone::from_name(unknown).unwrap().name, "UTC");
        }
        assert_eq!(
            TaskTimeZone::from_name("localtime").unwrap().name,
            "localtime"
        );
        assert_eq!(
            TaskTimeZone::from_name("posixrules").unwrap().name,
            "posixrules"
        );
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_project_list_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn setup() -> (Fixture, Uuid, Uuid, Uuid) {
        let f = Fixture::new().await;
        let (_, task) = f.task_attachment().await;
        let (project, status): (Vec<u8>, Vec<u8>) =
            sqlx::query_as("SELECT project_id,status_id FROM tasks WHERE id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let project = Uuid::from_slice(&project).unwrap();
        let status = Uuid::from_slice(&status).unwrap();
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,9223372036854775807)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(credential.to_string())
            .execute(&f.pool).await.unwrap();
        sqlx::query(
            "UPDATE tasks SET created_at=1760000000000000,updated_at=1760000000000001 WHERE id=?1",
        )
        .bind(task.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        (f, credential, project, status)
    }

    fn query(raw: Option<&str>, limit: i32) -> ParsedTaskListQuery {
        let mut query = crate::tasks::list_query::parse_task_list_query(
            raw,
            None,
            None,
            Some(limit),
            None,
            None,
        )
        .unwrap();
        query.as_of = crate::tasks::parse_iso_datetime("2026-10-06T00:00:00Z").unwrap();
        query
    }

    async fn list(
        f: &Fixture,
        credential: Uuid,
        project: Uuid,
        query: &ParsedTaskListQuery,
    ) -> TaskListPage {
        list_project_tasks_backend(&f.backend, f.workspace, project, f.user, credential, query)
            .await
            .unwrap()
            .unwrap()
    }

    async fn field(f: &Fixture, project: Uuid, kind: &str) -> (Uuid, Uuid) {
        let collection = Uuid::now_v7();
        let field = Uuid::now_v7();
        sqlx::query("INSERT INTO collections(id,workspace_id,project_id,kind,name) VALUES(?1,?2,?3,'task','List')")
            .bind(collection.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO collection_fields(id,workspace_id,collection_id,key,name,type,sort_key) VALUES(?1,?2,?3,'number','Field',?4,'V')")
            .bind(field.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice()).bind(kind).execute(&f.pool).await.unwrap();
        (collection, field)
    }

    #[tokio::test]
    async fn selected_task_list_gnu_text_pages_literal_filter_and_error_recovery() {
        gnu_text_pages_literal_filter_and_error_recovery(TaskScalarUse::Production).await;
    }

    #[tokio::test]
    async fn selected_task_list_reference_qualification_gnu_text_pages_literal_filter_and_error_recovery(
    ) {
        // The explicit pending envelope retains bootstrap refusal coverage
        // after the separately qualified production envelope is admitted.
        let pending = TaskScalarProfile::pending_envelope().unwrap();
        assert!(pending.runtime_profiles.is_empty());
        assert_eq!(
            pending.data_identity,
            TaskScalarProfile::compiled().unwrap().data_identity
        );
        gnu_text_pages_literal_filter_and_error_recovery(TaskScalarUse::ReferenceQualification)
            .await;
    }

    #[test]
    fn selected_scalar_qualified_runtime_admission_and_exact_drift_refusal() {
        let profile = TaskScalarProfile::compiled().unwrap();
        profile.require_runtime().unwrap();
        drop(TaskTextLocale::for_profile(profile).unwrap());
        let original: Value =
            serde_json::from_str(include_str!("task_scalar_pg18_profile.json")).unwrap();
        for (field, value, expected) in [
            (
                "glibc_version",
                "0.0".to_owned(),
                "Task GNU version mismatch",
            ),
            (
                "locale_archive_sha256",
                {
                    let mut hash = profile
                        .runtime_profiles
                        .iter()
                        .find(|runtime| runtime.arch == std::env::consts::ARCH)
                        .unwrap()
                        .locale_archive_sha256
                        .clone();
                    hash.replace_range(..1, if hash.starts_with('0') { "1" } else { "0" });
                    hash
                },
                "Task GNU locale archive mismatch",
            ),
        ] {
            // Negative drift only; keep the real receipt and reference/zones.
            let mut drift = original.clone();
            let runtime = drift["runtime_profiles"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|runtime| runtime["arch"] == std::env::consts::ARCH)
                .unwrap();
            runtime[field] = json!(value);
            let drift = TaskScalarProfile::decode(&drift.to_string()).unwrap();
            assert_eq!(drift.data_identity, profile.data_identity);
            assert!(matches!(
                drift.require_runtime(),
                Err(sqlx::Error::Protocol(message)) if message == expected
            ));
            assert!(matches!(
                TaskTextLocale::for_profile(&drift),
                Err(sqlx::Error::Protocol(message)) if message == expected
            ));
        }
    }

    #[test]
    fn selected_scalar_qualified_runtime_locpath_refusal_in_serial_child() {
        const SELECTOR: &str = "db::tasks::selected_project_list_tests::selected_scalar_qualified_runtime_locpath_refusal_in_serial_child";
        const CHILD_MARKER: &str = "FVOCI_SCALAR_LOCPATH_CHILD";
        let profile = TaskScalarProfile::compiled().unwrap();
        if let Some(marker) = std::env::var_os(CHILD_MARKER) {
            assert_eq!(marker, "serial-locpath-refusal");
            assert_eq!(
                std::env::var("LOCPATH").unwrap(),
                "/FVOCI-unqualified-test-locale"
            );
            assert!(matches!(
                profile.require_runtime(),
                Err(sqlx::Error::Protocol(message)) if message == "Task GNU locale search path unsupported"
            ));
            assert!(matches!(
                TaskTextLocale::for_profile(profile),
                Err(sqlx::Error::Protocol(message)) if message == "Task GNU locale search path unsupported"
            ));
        } else {
            profile.require_runtime().unwrap();
            drop(TaskTextLocale::for_profile(profile).unwrap());
            let executable = std::env::current_exe().unwrap();
            // Set only the fresh child's environment; no global env mutation.
            let child = std::process::Command::new(&executable)
                .args(["--exact", SELECTOR, "--test-threads=1"])
                .env(CHILD_MARKER, "serial-locpath-refusal")
                .env("LOCPATH", "/FVOCI-unqualified-test-locale")
                .output()
                .unwrap();
            // Optional caller-owned evidence output, never an admission input.
            if let Some(directory) = std::env::var_os("FVOCI_SCALAR_CHILD_RECEIPT_DIR") {
                use std::io::Write;
                use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
                let directory = std::path::PathBuf::from(directory);
                let temporary = std::env::temp_dir();
                assert!(directory.is_absolute() && temporary.is_absolute());
                assert_eq!(temporary.canonicalize().unwrap(), temporary);
                assert_eq!(directory.canonicalize().unwrap(), directory);
                assert!(directory.starts_with(&temporary) && directory != temporary);
                let metadata = std::fs::symlink_metadata(&directory).unwrap();
                assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
                // SAFETY: geteuid has no arguments or borrowed memory.
                let uid = unsafe { libc::geteuid() };
                assert_eq!(metadata.uid(), uid);
                assert_eq!(metadata.mode() & 0o777, 0o700);
                assert!(child.stdout.len() <= 65536 && child.stderr.len() <= 65536);
                let receipt = serde_json::to_vec_pretty(&json!({
                    "selector": SELECTOR,
                    "executable": executable,
                    "argv": ["--exact", SELECTOR, "--test-threads=1"],
                    "exit": child.status.code(),
                    "waited": true,
                    "positive_parent_admission_before_child": true,
                    "child_environment": {
                        "FVOCI_SCALAR_LOCPATH_CHILD": "serial-locpath-refusal",
                        "LOCPATH": "/FVOCI-unqualified-test-locale"
                    },
                    "stdout_sha256": task_scalar_sha256(&child.stdout),
                    "stderr_sha256": task_scalar_sha256(&child.stderr)
                }))
                .unwrap();
                for (name, bytes) in [
                    ("stdout", child.stdout.as_slice()),
                    ("stderr", child.stderr.as_slice()),
                    ("receipt.json", receipt.as_slice()),
                ] {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(directory.join(name))
                        .unwrap();
                    let metadata = file.metadata().unwrap();
                    assert!(metadata.is_file());
                    assert_eq!(metadata.uid(), uid);
                    assert_eq!(metadata.mode() & 0o777, 0o600);
                    file.write_all(bytes).unwrap();
                    file.sync_all().unwrap();
                }
            }
            assert!(child.status.success());
            let output = String::from_utf8(child.stdout).unwrap()
                + &String::from_utf8(child.stderr).unwrap();
            assert_eq!(
                output
                    .lines()
                    .filter(|line| *line == "running 1 test")
                    .count(),
                1
            );
            assert!(output
                .lines()
                .any(|line| line == format!("test {SELECTOR} ... ok")));
            assert_eq!(
                output
                    .lines()
                    .filter(|line| line.starts_with("test result:"))
                    .count(),
                1
            );
            assert!(output.contains("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured;"));
        }
    }

    async fn scalar_list(
        f: &Fixture,
        credential: Uuid,
        project: Uuid,
        query: &ParsedTaskListQuery,
        scalar_use: TaskScalarUse,
    ) -> TaskListPage {
        list_project_tasks_backend_with_use(
            &f.backend,
            f.workspace,
            project,
            f.user,
            credential,
            query,
            scalar_use,
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn gnu_text_pages_literal_filter_and_error_recovery(scalar_use: TaskScalarUse) {
        let (f, credential, project, status) = setup().await;
        if matches!(scalar_use, TaskScalarUse::ReferenceQualification) {
            assert!(matches!(
                list_project_tasks_backend_with_use(&f.backend, f.workspace, project, f.user, credential, &query(None, 7), TaskScalarUse::PendingEnvelopeProduction).await,
                Err(sqlx::Error::Protocol(message)) if message == "Task scalar runtime qualification unavailable"
            ));
        }
        // Same authorization bridge before any scalar admission or projection.
        assert!(matches!(
            list_project_tasks_backend_with_use(
                &f.backend,
                f.workspace,
                project,
                f.user,
                Uuid::now_v7(),
                &query(None, 7),
                scalar_use
            )
            .await,
            Ok(Err(ProjectDbError::Forbidden))
        ));
        assert!(matches!(
            list_project_tasks_backend_with_use(
                &f.backend,
                f.workspace,
                Uuid::now_v7(),
                f.user,
                credential,
                &query(None, 7),
                scalar_use
            )
            .await,
            Ok(Err(ProjectDbError::NotFound))
        ));
        let (collection, field) = field(&f, project, "text").await;
        // Actual PG S10 order. IDs scan in C order, not this expected order;
        // 48 scalar rows + one NULL seed force more than one batch/page.
        let default = [
            "e", "E", "e\u{301}", "é", "i", "I", "İ", "ı", "ß", "ẞ", "z", "Z", "σ", "Σ", "ς", "中",
        ];
        let c = [
            "E", "I", "Z", "e", "e\u{301}", "i", "z", "ß", "é", "İ", "ı", "Σ", "ς", "σ", "ẞ", "中",
        ];
        for (index, text) in c.iter().enumerate() {
            for repeat in 0..3 {
                let id = Uuid::from_u128((index * 3 + repeat + 1) as u128);
                let item = Uuid::now_v7();
                sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,'{}',?7,1760000000000000,1760000000000001)")
                    .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(index as i32*3+repeat as i32+2).bind(text).bind(status.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
                sqlx::query("INSERT INTO collection_items(id,workspace_id,collection_id,task_id) VALUES(?1,?2,?3,?4)")
                    .bind(item.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
                sqlx::query("INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_text) VALUES(?1,?2,?3,?4,'text',?5)")
                    .bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice()).bind(item.as_bytes().as_slice()).bind(field.as_bytes().as_slice()).bind(text).execute(&f.pool).await.unwrap();
            }
        }
        let seed: Vec<u8> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE project_id=?1 AND number=1")
                .bind(project.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        for direction in ["asc", "desc"] {
            let raw = json!({"sort":[{"field":field,"direction":direction}]}).to_string();
            let mut request = query(Some(&raw), 7);
            let mut actual = Vec::new();
            loop {
                let page = scalar_list(&f, credential, project, &request, scalar_use).await;
                assert_eq!(page.status_counts, vec![(status, 49)]);
                actual.extend(page.items.iter().map(|item| item.meta.id));
                assert!(actual.len() <= 49, "duplicate/nonadvancing pagination");
                let Some(cursor) = page.next_cursor else {
                    break;
                };
                request.cursor = Some(crate::tasks::list_query::decode_cursor(&cursor).unwrap());
            }
            let mut ordered = default.to_vec();
            if direction == "desc" {
                ordered.reverse();
            }
            let mut expected = Vec::new();
            for text in ordered {
                let index = c.iter().position(|value| *value == text).unwrap();
                expected
                    .extend((0..3).map(|repeat| Uuid::from_u128((index * 3 + repeat + 1) as u128)));
            }
            expected.push(Uuid::from_slice(&seed).unwrap());
            assert_eq!(actual, expected);
        }
        // Reverse dotted-I and non-contextual sigma exercise the real filter,
        // not just a direct case-mapper assertion.
        for (needle, count) in [("İ", 9), ("Σ", 6), ("%_\\", 0)] {
            let raw = json!({"filters":{"title":needle}}).to_string();
            let page =
                scalar_list(&f, credential, project, &query(Some(&raw), 50), scalar_use).await;
            assert_eq!(page.items.len(), count);
            assert_eq!(
                page.status_counts
                    .iter()
                    .map(|(_, count)| *count)
                    .sum::<i64>(),
                count as i64
            );
        }
        let invalid = sqlx::query("UPDATE collection_values SET value_text=?1 WHERE item_id=(SELECT id FROM collection_items WHERE task_id=?2)")
            .bind("bad\0text").bind(Uuid::from_u128(1).as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(invalid.rows_affected(), 1);
        let raw = json!({"sort":[{"field":field,"direction":"asc"}]}).to_string();
        assert!(
            matches!(list_project_tasks_backend_with_use(&f.backend, f.workspace, project, f.user, credential, &query(Some(&raw), 7), scalar_use).await,
            Err(sqlx::Error::Protocol(message)) if message == "Task scalar text contains NUL")
        );
        sqlx::query("UPDATE collection_values SET value_text='E' WHERE item_id=(SELECT id FROM collection_items WHERE task_id=?1)").bind(Uuid::from_u128(1).as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            scalar_list(&f, credential, project, &query(Some(&raw), 100), scalar_use)
                .await
                .items
                .len(),
            49
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn selected_task_list_exhaustive_batches_exact_decimal_count_cursor_and_hydration() {
        let (f, credential, project, status) = setup().await;
        let (collection, field) = field(&f, project, "number").await;
        let label = Uuid::now_v7();
        sqlx::query("INSERT INTO labels(id,workspace_id,project_id,name,color) VALUES(?1,?2,?3,'attached','blue')")
            .bind(label.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        // IDs scan in ascending insertion index; numeric sort runs in reverse,
        // so the first page cannot be obtained by stopping after one batch.
        for index in 1..=70u128 {
            let id = Uuid::from_u128(index);
            let item = Uuid::now_v7();
            let number = if index == 69 {
                "9007199254740992".to_owned()
            } else if index == 70 {
                "9007199254740993".to_owned()
            } else {
                (100 - index).to_string()
            };
            sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by,created_at,updated_at,recurrence) VALUES(?1,?2,?3,?4,'literal %_\\ 😀',?5,'{}',?6,1760000000000000,1760000000000001,'{\"frequency\":\"weekly\"}')")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(index as i32+1).bind(status.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO collection_items(id,workspace_id,collection_id,task_id) VALUES(?1,?2,?3,?4)")
                .bind(item.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_number) VALUES(?1,?2,?3,?4,'number',?5)")
                .bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice()).bind(item.as_bytes().as_slice()).bind(field.as_bytes().as_slice()).bind(number).execute(&f.pool).await.unwrap();
            sqlx::query(
                "INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(id.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO task_labels(workspace_id,task_id,label_id) VALUES(?1,?2,?3)")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(id.as_bytes().as_slice())
                .bind(label.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        // The fixture uses the maintained STRICT/FK schema: a made-up field
        // cannot be smuggled into projection rows, and the failed statement
        // must leave the healthy populated list intact.
        let first_item: Vec<u8> =
            sqlx::query_scalar("SELECT id FROM collection_items WHERE task_id=?1")
                .bind(Uuid::from_u128(1).as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let fk = sqlx::query("INSERT INTO collection_values(workspace_id,collection_id,item_id,field_id,field_type,value_number) VALUES(?1,?2,?3,?4,'number','1')")
            .bind(f.workspace.as_bytes().as_slice()).bind(collection.as_bytes().as_slice())
            .bind(first_item).bind(Uuid::now_v7().as_bytes().as_slice()).execute(&f.pool).await;
        assert!(fk
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .is_some_and(|error| error.is_foreign_key_violation()));
        let raw = json!({"sort":[{"field":field,"direction":"asc"}]}).to_string();
        let mut request = query(Some(&raw), 7);
        let mut actual = Vec::new();
        let mut first_cursor = None;
        loop {
            let page = list(&f, credential, project, &request).await;
            assert_eq!(page.status_counts, vec![(status, 71)]);
            for item in &page.items {
                if item.meta.id.as_u128() <= 70 {
                    assert_eq!(item.assignee_ids, vec![f.user]);
                    assert_eq!(item.label_ids, vec![label]);
                    assert_eq!(item.meta.recurrence, Some(json!({"frequency":"weekly"})));
                }
            }
            actual.extend(page.items.iter().map(|item| item.meta.id));
            let Some(cursor) = page.next_cursor else {
                break;
            };
            if first_cursor.is_none() {
                first_cursor = Some(cursor.clone());
            }
            request.cursor = Some(crate::tasks::list_query::decode_cursor(&cursor).unwrap());
            assert!(actual.len() <= 71, "duplicate/nonadvancing pagination");
        }
        let seed: Vec<u8> =
            sqlx::query_scalar("SELECT id FROM tasks WHERE project_id=?1 AND number=1")
                .bind(project.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let mut expected = (1..=68u128).rev().map(Uuid::from_u128).collect::<Vec<_>>();
        expected.extend([
            Uuid::from_u128(69),
            Uuid::from_u128(70),
            Uuid::from_slice(&seed).unwrap(),
        ]);
        assert_eq!(actual, expected);
        let exact=json!({"filters":{"custom":[{"fieldId":field,"operator":"equals","value":9007199254740992u64}]}}).to_string();
        let page = list(&f, credential, project, &query(Some(&exact), 50)).await;
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.meta.id)
                .collect::<Vec<_>>(),
            vec![Uuid::from_u128(69)]
        );
        assert_eq!(page.status_counts, vec![(status, 1)]);
        // Cursor key must be re-evaluated, not trusted from the client.
        let mut stale = query(Some(&raw), 7);
        stale.cursor =
            Some(crate::tasks::list_query::decode_cursor(&first_cursor.unwrap()).unwrap());
        sqlx::query("UPDATE collection_values SET value_number='999' WHERE item_id=(SELECT id FROM collection_items WHERE task_id=?1)")
            .bind(stale.cursor.as_ref().unwrap().id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &stale
            )
            .await
            .unwrap(),
            Err(ProjectDbError::InvalidCursor)
        ));
        assert_eq!(
            list(&f, credential, project, &query(None, 100))
                .await
                .items
                .len(),
            71
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn selected_task_list_current_authority_refusals_restore_healthy_progress() {
        let (f, credential, project, _) = setup().await;
        let request = query(None, 50);
        assert_eq!(list(&f, credential, project, &request).await.items.len(), 1);
        for (workspace, target, actor, session, forbidden) in [
            (Uuid::now_v7(), project, f.user, credential, false),
            (f.workspace, Uuid::now_v7(), f.user, credential, false),
            (f.workspace, project, Uuid::now_v7(), credential, true),
            (f.workspace, project, f.user, Uuid::now_v7(), true),
        ] {
            let result =
                list_project_tasks_backend(&f.backend, workspace, target, actor, session, &request)
                    .await
                    .unwrap();
            assert!(if forbidden {
                matches!(result, Err(ProjectDbError::Forbidden))
            } else {
                matches!(result, Err(ProjectDbError::NotFound))
            });
        }
        sqlx::query("UPDATE projects SET visibility='private' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &request
            )
            .await
            .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        let grant = Uuid::now_v7();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(grant.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(list(&f, credential, project, &request).await.items.len(), 1);
        sqlx::query("DELETE FROM project_members WHERE id=?1")
            .bind(grant.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &request
            )
            .await
            .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE projects SET visibility='workspace' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for (sql, restore, forbidden) in [
            (
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                "UPDATE sessions SET revoked_at=NULL WHERE id=?1",
                true,
            ),
            (
                "UPDATE projects SET deleted_at=1 WHERE id=?1",
                "UPDATE projects SET deleted_at=NULL WHERE id=?1",
                false,
            ),
        ] {
            let id = if forbidden { credential } else { project };
            sqlx::query(sql)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let result = list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &request,
            )
            .await
            .unwrap();
            assert!(if forbidden {
                matches!(result, Err(ProjectDbError::Forbidden))
            } else {
                matches!(result, Err(ProjectDbError::NotFound))
            });
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(list(&f, credential, project, &request).await.items.len(), 1);
        }
        // No write-side products/outbox effects are introduced by refusal/read.
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE project_id=?1")
                .bind(project.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn selected_task_list_catalog_reference_and_current_schema_cursor_refusals() {
        let (f, credential, project, _) = setup().await;
        let (collection, field) = field(&f, project, "user_multi").await;
        let wrong = Uuid::now_v7();
        for raw in [
            json!({"filters":{"custom":[{"fieldId":wrong,"operator":"empty"}]}}),
            json!({"filters":{"custom":[{"fieldId":field,"operator":"equals","value":wrong}]}}),
            json!({"sort":[{"field":field,"direction":"asc"}]}),
        ] {
            assert!(matches!(
                list_project_tasks_backend(
                    &f.backend,
                    f.workspace,
                    project,
                    f.user,
                    credential,
                    &query(Some(&raw.to_string()), 50)
                )
                .await
                .unwrap(),
                Err(ProjectDbError::InvalidInput)
            ));
        }
        let raw = json!({"filters":{"custom":[{"fieldId":field,"operator":"empty"}]}}).to_string();
        assert_eq!(
            list(&f, credential, project, &query(Some(&raw), 50))
                .await
                .items
                .len(),
            1
        );
        sqlx::query("UPDATE collections SET deleted_at=1 WHERE id=?1")
            .bind(collection.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &query(Some(&raw), 50)
            )
            .await
            .unwrap(),
            Err(ProjectDbError::InvalidInput)
        ));
        sqlx::query("UPDATE collections SET deleted_at=NULL WHERE id=?1")
            .bind(collection.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list(&f, credential, project, &query(Some(&raw), 50))
                .await
                .items
                .len(),
            1
        );
        let extra = Uuid::now_v7();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by,created_at,updated_at) SELECT ?1,workspace_id,project_id,2,'second',status_id,content_json,created_by,created_at,updated_at FROM tasks WHERE project_id=?2 AND number=1")
            .bind(extra.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut continuation = query(Some(&raw), 1);
        let first = list(&f, credential, project, &continuation).await;
        assert_eq!(
            first
                .status_counts
                .iter()
                .map(|(_, count)| count)
                .sum::<i64>(),
            2
        );
        continuation.cursor =
            Some(crate::tasks::list_query::decode_cursor(&first.next_cursor.unwrap()).unwrap());
        sqlx::query("UPDATE collection_fields SET version=version+1 WHERE id=?1")
            .bind(field.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &continuation
            )
            .await
            .unwrap(),
            Err(ProjectDbError::InvalidCursor)
        ));
        assert_eq!(
            list(&f, credential, project, &query(Some(&raw), 50))
                .await
                .items
                .len(),
            2
        );
        let badlabel = json!({"filters":{"labelId":wrong}}).to_string();
        assert!(matches!(
            list_project_tasks_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                &query(Some(&badlabel), 50)
            )
            .await
            .unwrap(),
            Err(ProjectDbError::InvalidInput)
        ));
        f.close().await;
    }
}
