use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use sqlx::{PgPool, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{begin_read, session_is_live, set_tenant};
use crate::db::holidays::list_holiday_dates;
use crate::db::projects::{project_permission_by_id, ProjectDbError};
use crate::db::tasks::{
    compiled_sort_terms, load_task_refs, order_clause, task_list_filter_conditions,
};
use crate::db::view_query::{compile_view_query, CompileOptions, RootKind, SqlArgs, ViewScope};
use crate::gantt::month_range;
use crate::gantt::{
    prepare_gantt, GanttLayoutItemOutput, GanttLayoutOutput, GanttLinkInput, GanttTaskInput,
    LinkType, PrepareInput, ScheduleInference,
};
use crate::projects::ProjectPermission;
use crate::tasks::dependency::finish_date;
use crate::tasks::layout_query::ParsedTaskLayoutQuery;
use crate::tasks::list_query::{effective_sort_entries, ParsedTaskListQuery};

pub struct TaskLayoutRow {
    pub id: Uuid,
    pub title: String,
    pub number: i32,
    pub status_id: Uuid,
    pub priority: String,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
}

pub async fn get_project_task_layout(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    layout: &ParsedTaskLayoutQuery,
    list_query: &ParsedTaskListQuery,
) -> Result<Result<GanttLayoutOutput, ProjectDbError>, sqlx::Error> {
    let range = month_range(layout.year, layout.month, layout.week_starts_on)
        .ok_or_else(|| sqlx::Error::RowNotFound)?;
    let scale_start = range.0.clone();
    let to = range.1.clone();
    let time_zone = crate::db::dashboard::user_time_zone(pool, actor_user_id).await?;

    let mut tx = begin_read(pool).await?;
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !crate::db::documents::workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let permission =
        project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id).await?;
    if !permission
        .map(|p| p.at_least(ProjectPermission::View))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    let scope_condition = "t.project_id = $2".to_string();
    let (mut base_conditions, mut base_binds) =
        task_list_filter_conditions(list_query, actor_user_id, scope_condition);
    let mut compiled_args = SqlArgs::starting_at(base_binds.len() + 3);
    let compiled = match compile_view_query(
        &mut tx,
        ViewScope {
            workspace_id,
            project_id: Some(project_id),
            collection_id: None,
            kind: RootKind::Task,
        },
        &list_query.view,
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
        Ok(c) => c,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    };
    base_conditions.extend(compiled.conditions.iter().cloned());
    base_binds.extend(compiled_args.values.iter().cloned());
    let compiled_sorts = compiled_sort_terms(&compiled);
    let sort = effective_sort_entries(&list_query.view.sort);
    let order_sql = order_clause(&sort, &compiled_sorts);
    let where_sql = base_conditions.join(" AND ");
    let list_sql = format!(
        r#"
        SELECT t.id, t.title, t.number, t.status_id, t.priority,
               t.start_date, t.due_date, t.due_at
        FROM fvoci.tasks t
        WHERE {where_sql}
        ORDER BY {order_sql}
        LIMIT 501
        "#
    );
    let mut q = sqlx::query(&list_sql).bind(workspace_id).bind(project_id);
    for v in &base_binds {
        q = q.bind(v);
    }
    let rows = q.fetch_all(&mut *tx).await?;
    let truncated = rows.len() > 500;
    let rows = rows.into_iter().take(500).collect::<Vec<_>>();

    let items: Vec<TaskLayoutRow> = rows
        .iter()
        .map(|row| TaskLayoutRow {
            id: row.get("id"),
            title: row.get("title"),
            number: row.get("number"),
            status_id: row.get("status_id"),
            priority: row.get("priority"),
            start_date: row.get("start_date"),
            due_date: row.get("due_date"),
            due_at: row.get("due_at"),
        })
        .collect();
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let (assignee_map, _) = load_task_refs(&mut tx, workspace_id, &ids).await?;
    let links = list_dependencies_among(&mut tx, workspace_id, &ids).await?;
    let holidays = list_holiday_dates(&mut tx, workspace_id).await?;
    tx.commit().await?;

    let holiday_strings: Vec<String> = holidays
        .into_iter()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .collect();

    let gantt_tasks: Vec<GanttTaskInput> = items
        .iter()
        .map(|item| {
            let due = finish_date(item.due_date, item.due_at);
            GanttTaskInput {
                id: item.id,
                title: item.title.clone(),
                start: item.start_date.map(|d| d.format("%Y-%m-%d").to_string()),
                due: due.map(|d| d.format("%Y-%m-%d").to_string()),
                milestone: false,
            }
        })
        .collect();

    let item_meta: Vec<GanttLayoutItemOutput> = items
        .iter()
        .map(|item| {
            let due = finish_date(item.due_date, item.due_at);
            let start = item
                .start_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .or_else(|| due.map(|d| d.format("%Y-%m-%d").to_string()))
                .unwrap_or_default();
            let end = due
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| start.clone());
            GanttLayoutItemOutput {
                id: item.id.to_string(),
                title: item.title.clone(),
                number: item.number,
                status_id: item.status_id.to_string(),
                priority: item.priority.clone(),
                assignee_ids: assignee_map
                    .get(&item.id)
                    .map(|ids| ids.iter().map(|id| id.to_string()).collect())
                    .unwrap_or_default(),
                start_date: item.start_date.map(|d| d.format("%Y-%m-%d").to_string()),
                due_date: item.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
                due_at: item
                    .due_at
                    .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true)),
                start,
                end,
                milestone: false,
                inferred: ScheduleInference::None,
            }
        })
        .collect();

    let gantt_links: Vec<GanttLinkInput> = links
        .into_iter()
        .filter_map(|l| {
            Some(GanttLinkInput {
                blocker_id: l.blocker_id,
                blocked_id: l.blocked_id,
                link_type: LinkType::parse(&l.dependency_type)?,
                lag_days: l.lag_days,
            })
        })
        .collect();

    Ok(Ok(prepare_gantt(PrepareInput {
        tasks: gantt_tasks,
        links: gantt_links,
        holidays: holiday_strings,
        scale_start,
        scale_end: to,
        zoom: layout.zoom,
        px_per_day: layout.px_per_day,
        lane_height: layout.lane_height,
        pack: layout.pack,
        max_lanes: layout.max_lanes,
        truncated,
        item_meta,
    })))
}

struct DepRow {
    blocker_id: Uuid,
    blocked_id: Uuid,
    dependency_type: String,
    lag_days: i32,
}

async fn list_dependencies_among(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    task_ids: &[Uuid],
) -> Result<Vec<DepRow>, sqlx::Error> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, i32)>(
        r#"
        SELECT blocker_id, blocked_id, type, lag_days
        FROM fvoci.task_dependencies
        WHERE workspace_id = $1
          AND blocker_id = ANY($2)
          AND blocked_id = ANY($2)
        "#,
    )
    .bind(workspace_id)
    .bind(task_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(blocker_id, blocked_id, dependency_type, lag_days)| DepRow {
                blocker_id,
                blocked_id,
                dependency_type,
                lag_days,
            },
        )
        .collect())
}
