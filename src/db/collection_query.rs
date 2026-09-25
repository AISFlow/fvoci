//! Collection query (source `repos/collection-query.ts` + `queryCollection`).
//!
//! One REPEATABLE READ, READ ONLY transaction. The visible row set is a CTE
//! over the collection's items joined to their live documents/tasks, filtered
//! by the actor's current read access and the compiled view query. Paging is
//! keyset over the view's sort tuple with `id` as the unique tie-breaker; the
//! cursor carries the anchor id, a SHA-256 of the anchor's sort tuple and a
//! fingerprint of the query scope, never the sort values themselves.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::collections::{CollectionKind, DateBy, FieldType, GroupBy, QueryInput};
use crate::db::collections::{
    begin_member, load_fields, require_collection, validate_query_config, Actor, CollectionDbError,
    DbResult, Need,
};
use crate::db::documents::document_permission;
use crate::db::view_query::{
    after_sort_tuple, compile_view_query, due_date_sql, scalar_value_sql, sort_tuple_hash_sql,
    CompileOptions, SqlArgs, ViewScope,
};
use crate::projects::{workspace_base_permission, ProjectPermission};
use crate::search::query::load_search_acl;
use crate::tasks::list_query::{decode_cursor, encode_cursor, TaskListCursor};

/// Month cells show at most three titles; `days[].count` carries the total.
pub const CALENDAR_PREVIEW_PER_DAY: i64 = 3;
/// Source `QUERY_RESPONSE_BYTES`: the page shrinks rather than exceed 8 MiB.
pub const QUERY_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct QueryRow {
    pub id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub display_id: String,
    pub title: String,
    pub task_type: Option<String>,
    pub status_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub version: i32,
    pub group: Option<String>,
    pub date: Option<NaiveDate>,
    pub can_edit: bool,
    pub values: Vec<(Uuid, Value)>,
}

#[derive(Debug, Clone)]
pub struct QueryGroup {
    pub id: Option<String>,
    pub name: String,
    pub item_ids: Vec<Uuid>,
    pub count: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub can_edit: bool,
    pub days: Vec<(Option<NaiveDate>, i64)>,
    pub items: Vec<QueryRow>,
    pub groups: Vec<QueryGroup>,
    pub count: i64,
    pub next_cursor: Option<String>,
    pub previews: Vec<QueryRow>,
}

const ROW_COLUMNS: &str = "id, document_id, task_id, display_id, title, task_type, status_id, \
     start_date, due_date, due_at, version, grp, day";

fn row_from(row: &sqlx::postgres::PgRow) -> Result<QueryRow, sqlx::Error> {
    Ok(QueryRow {
        id: row.try_get("id")?,
        document_id: row.try_get("document_id")?,
        task_id: row.try_get("task_id")?,
        display_id: row.try_get("display_id")?,
        title: row.try_get("title")?,
        task_type: row.try_get("task_type")?,
        status_id: row.try_get("status_id")?,
        start_date: row.try_get("start_date")?,
        due_date: row.try_get("due_date")?,
        due_at: row.try_get("due_at")?,
        version: row.try_get("version")?,
        group: row.try_get("grp")?,
        date: row.try_get("day")?,
        can_edit: false,
        values: Vec::new(),
    })
}

fn bind_all<'q>(
    sql: &'q str,
    args: &'q [String],
) -> sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments> {
    let mut query = sqlx::query(sql);
    for value in args {
        query = query.bind(value);
    }
    query
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// Value JSON per (item, field), same shapes as the `collectionValue` union.
/// Values of the given items as `(item_id, field_id, value)`.
pub(crate) async fn load_item_values(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, Value)>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, Option<Value>)> = sqlx::query_as(&values_sql("$2"))
        .bind(workspace_id)
        .bind(ids)
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(item, field, value)| value.map(|value| (item, field, value)))
        .collect())
}

fn values_sql(ids_ph: &str) -> String {
    format!(
        r#"
        SELECT item_id, field_id, CASE field_type
            WHEN 'text' THEN jsonb_build_object('text', value_text)
            WHEN 'paragraph' THEN jsonb_build_object('text', value_text)
            WHEN 'number' THEN jsonb_build_object('number', value_number)
            WHEN 'date' THEN jsonb_build_object('date', value_date)
            WHEN 'datetime' THEN jsonb_build_object('datetime',
                to_char(value_ts AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"'))
            WHEN 'checkbox' THEN jsonb_build_object('checkbox', value_bool)
        END AS value
        FROM fvoci.collection_values WHERE workspace_id = $1 AND item_id = ANY({ids_ph})
        UNION ALL
        SELECT item_id, field_id, jsonb_build_object('options', jsonb_agg(option_id ORDER BY option_id))
        FROM fvoci.collection_choices WHERE workspace_id = $1 AND item_id = ANY({ids_ph})
        GROUP BY item_id, field_id
        UNION ALL
        SELECT item_id, field_id, jsonb_build_object('users', jsonb_agg(user_id ORDER BY user_id))
        FROM fvoci.collection_people WHERE workspace_id = $1 AND item_id = ANY({ids_ph})
        GROUP BY item_id, field_id
        "#
    )
}

pub async fn query_collection(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    q: &QueryInput,
) -> DbResult<QueryResult> {
    let cursor = match &q.cursor {
        None => None,
        Some(raw) => match decode_cursor(raw) {
            Ok(cursor) => Some(cursor),
            Err(_) => return Ok(Err(CollectionDbError::InvalidCursor)),
        },
    };
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    // Bounds the correlated filter/sort subqueries of one user-built query.
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *tx)
        .await?;
    let result = run(&mut tx, workspace_id, actor, collection_id, q, cursor).await?;
    match result {
        Ok(out) => {
            tx.commit().await?;
            Ok(Ok(out))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

async fn run(
    tx: &mut Transaction<'_, Postgres>,
    ws: Uuid,
    actor: &Actor,
    collection_id: Uuid,
    q: &QueryInput,
    cursor: Option<TaskListCursor>,
) -> DbResult<QueryResult> {
    let role = match begin_member(tx, ws, actor, false).await? {
        Ok(role) => role,
        Err(err) => return Ok(Err(err)),
    };
    let (collection, access) =
        match require_collection(tx, ws, actor, role, collection_id, Need::Read).await? {
            Ok(found) => found,
            Err(err) => return Ok(Err(err)),
        };
    let fields = load_fields(tx, ws, collection_id).await?;
    if let Err(err) = validate_query_config(
        tx,
        &collection,
        &fields,
        &q.config,
        q.day.is_some(),
        q.window.as_ref(),
    )
    .await?
    {
        return Ok(Err(err));
    }
    let time_zone: String = sqlx::query_scalar("SELECT timezone FROM fvoci.users WHERE id = $1")
        .bind(actor.user_id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or_else(|| "UTC".to_string());
    let as_of = cursor.as_ref().map(|c| c.as_of).unwrap_or_else(Utc::now);
    let acl = load_search_acl(tx, ws, actor.user_id, role, None).await?;
    let task = collection.kind == CollectionKind::Task;

    // $1 workspace, $2 collection; everything else is appended in order.
    let mut args = SqlArgs::starting_at(1);
    args.push(ws.to_string());
    args.push(collection_id.to_string());
    let projects_ph = args.push_uuid_array(&acl.project_ids);
    let read_sql = if task {
        format!("r.project_id = ANY({projects_ph})")
    } else {
        let include_ph = args.push(acl.include_wiki.to_string());
        let wiki_ph = args.push_uuid_array(&acl.wiki_document_ids);
        format!(
            "((r.project_id IS NOT NULL AND r.project_id = ANY({projects_ph})) \
             OR (r.project_id IS NULL AND ({include_ph}::boolean OR r.id = ANY({wiki_ph}))))"
        )
    };
    let compiled = match compile_view_query(
        tx,
        ViewScope {
            workspace_id: ws,
            project_id: collection.project_id,
            collection_id: Some(collection_id),
            kind: collection.root_kind(),
        },
        &q.config.query,
        &CompileOptions {
            actor_user_id: actor.user_id,
            time_zone: &time_zone,
            standard_filters: true,
        },
        "r",
        &mut args,
    )
    .await?
    {
        Ok(compiled) => compiled,
        Err(_) => return Ok(Err(CollectionDbError::InvalidInput)),
    };

    let window_tz = q
        .window
        .as_ref()
        .map(|w| w.time_zone.clone())
        .unwrap_or_else(|| "UTC".to_string());
    let group_sql = match q.config.group_by {
        None => "NULL::text".to_string(),
        Some(GroupBy::Status) => "r.status_id::text".to_string(),
        Some(GroupBy::Field(id)) => format!(
            "(SELECT ch.option_id::text FROM fvoci.collection_choices ch \
             WHERE ch.workspace_id = i.workspace_id AND ch.collection_id = i.collection_id \
             AND ch.item_id = i.id AND ch.field_id = {}::uuid LIMIT 1)",
            args.push(id.to_string())
        ),
    };
    let date_field_type = match q.config.date_by {
        Some(DateBy::Field(id)) => fields
            .iter()
            .find(|f| f.id == id)
            .map(|f| (id, f.field_type)),
        _ => None,
    };
    let date_sql = match q.config.date_by {
        None => "NULL::date".to_string(),
        Some(DateBy::Due) => due_date_sql("r", &args.push(window_tz.clone())),
        Some(DateBy::Start) => "r.start_date".to_string(),
        Some(DateBy::Field(id)) => {
            let field_ph = args.push(id.to_string());
            match date_field_type {
                Some((_, FieldType::Datetime)) => format!(
                    "({} AT TIME ZONE {})::date",
                    scalar_value_sql(collection.root_kind(), "r", &field_ph, "value_ts"),
                    args.push(window_tz.clone())
                ),
                _ => scalar_value_sql(collection.root_kind(), "r", &field_ph, "value_date"),
            }
        }
    };
    let ordering: Vec<(String, bool)> = compiled
        .order
        .iter()
        .enumerate()
        .map(|(index, term)| (format!("ordering_{index}"), term.desc))
        .collect();
    let ordering_select = compiled
        .order
        .iter()
        .enumerate()
        .map(|(index, term)| format!("{} AS ordering_{index}", term.sql("r")))
        .collect::<Vec<_>>()
        .join(", ");
    let (root_table, root_column) = if task {
        ("tasks", "task_id")
    } else {
        ("documents", "document_id")
    };
    let task_cols = if task {
        "r.type AS task_type, r.status_id AS status_id, r.start_date AS start_date, \
         r.due_date AS due_date, r.due_at AS due_at"
    } else {
        "NULL::text AS task_type, NULL::uuid AS status_id, NULL::date AS start_date, \
         NULL::date AS due_date, NULL::timestamptz AS due_at"
    };
    let window_sql = match &q.window {
        None => "true".to_string(),
        Some(window) => format!(
            "day IS NULL OR (day >= {}::date AND day < {}::date)",
            args.push(window.from.to_string()),
            args.push(window.to.to_string())
        ),
    };
    let source = format!(
        r#"
        WITH source AS (
            SELECT i.id, i.document_id, i.task_id,
                   COALESCE(p.key, 'WIKI') || '-' || r.number AS display_id,
                   r.title, i.version, {task_cols},
                   {group_sql} AS grp, {date_sql} AS day, {ordering_select}
            FROM fvoci.collection_items i
            JOIN fvoci.{root_table} r ON r.workspace_id = i.workspace_id AND r.id = i.{root_column}
            LEFT JOIN fvoci.projects p ON p.workspace_id = r.workspace_id AND p.id = r.project_id
            WHERE i.workspace_id = $1::uuid AND i.collection_id = $2::uuid
              AND r.deleted_at IS NULL {archived}
              AND {read_sql}
              AND {where_sql}
        ), visible AS (SELECT * FROM source WHERE {window_sql})
        "#,
        archived = if task {
            "AND r.archived_at IS NULL"
        } else {
            ""
        },
        where_sql = compiled.where_sql(),
    );

    let count: i64 = {
        let sql = format!("{source} SELECT count(*) FROM visible");
        bind_all(&sql, &args.values)
            .fetch_one(&mut **tx)
            .await?
            .try_get(0)?
    };
    let buckets: Vec<(Option<String>, i64)> = {
        let sql = format!("{source} SELECT grp, count(*) FROM visible GROUP BY grp");
        bind_all(&sql, &args.values)
            .fetch_all(&mut **tx)
            .await?
            .iter()
            .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
            .collect::<Result<_, sqlx::Error>>()?
    };

    let mut days = Vec::new();
    let mut previews: Vec<QueryRow> = Vec::new();
    let preview_order = ordering
        .iter()
        .map(|(alias, desc)| format!("{alias} {} NULLS LAST", if *desc { "DESC" } else { "ASC" }))
        .chain(std::iter::once("id ASC".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    if let Some(window) = &q.window {
        let mut day_args = args.clone();
        let from_ph = day_args.push(window.from.to_string());
        let to_ph = day_args.push(window.to.to_string());
        let sql = format!(
            "{source}, days AS (
                SELECT generate_series({from_ph}::timestamp, {to_ph}::timestamp - interval '1 day', interval '1 day')::date AS date
                UNION ALL SELECT NULL::date
            ), counts AS (SELECT day, count(*) AS c FROM visible GROUP BY day)
            SELECT days.date, COALESCE(counts.c, 0) FROM days
            LEFT JOIN counts ON counts.day IS NOT DISTINCT FROM days.date
            ORDER BY days.date NULLS LAST"
        );
        for row in bind_all(&sql, &day_args.values)
            .fetch_all(&mut **tx)
            .await?
        {
            days.push((row.try_get(0)?, row.try_get(1)?));
        }
        let sql = format!(
            "{source} SELECT {ROW_COLUMNS} FROM (
                SELECT *, row_number() OVER (PARTITION BY day ORDER BY {preview_order}) AS rn FROM visible
            ) ranked WHERE rn <= {CALENDAR_PREVIEW_PER_DAY} ORDER BY day NULLS LAST, rn, id"
        );
        for row in bind_all(&sql, &args.values).fetch_all(&mut **tx).await? {
            previews.push(row_from(&row)?);
        }
        if let (Some((field_id, _)), false) = (date_field_type, previews.is_empty()) {
            let ids: Vec<Uuid> = previews.iter().map(|p| p.id).collect();
            let rows: Vec<(Uuid, Uuid, Option<Value>)> = sqlx::query_as(&format!(
                "SELECT * FROM ({}) v WHERE field_id = $3",
                values_sql("$2")
            ))
            .bind(ws)
            .bind(&ids)
            .bind(field_id)
            .fetch_all(&mut **tx)
            .await?;
            for (item_id, field, value) in rows {
                if let (Some(preview), Some(value)) =
                    (previews.iter_mut().find(|p| p.id == item_id), value)
                {
                    preview.values.push((field, value));
                }
            }
        }
    }

    // Group catalog: workflow statuses, or the select field's options + "none".
    let mut catalog: Vec<(Option<String>, String, bool)> = Vec::new();
    match q.config.group_by {
        Some(GroupBy::Status) => {
            let rows: Vec<(Uuid, String)> = sqlx::query_as(
                r#"SELECT id, name FROM fvoci.statuses WHERE workspace_id = $1 AND project_id = $2
                   ORDER BY sort_key COLLATE "C", id"#,
            )
            .bind(ws)
            .bind(collection.project_id)
            .fetch_all(&mut **tx)
            .await?;
            catalog.extend(
                rows.into_iter()
                    .map(|(id, name)| (Some(id.to_string()), name, false)),
            );
        }
        Some(GroupBy::Field(field_id)) => {
            if let Some(field) = fields.iter().find(|f| f.id == field_id) {
                catalog.extend(field.options.iter().map(|o| {
                    (
                        Some(o.id.to_string()),
                        o.label.clone(),
                        o.deleted_at.is_some(),
                    )
                }));
            }
        }
        None => {}
    }
    if q.config.group_by != Some(GroupBy::Status) {
        catalog.push((None, String::new(), false));
    }

    let mut page_args = args.clone();
    let selected = match &q.group {
        None => "true".to_string(),
        Some(None) => "grp IS NULL".to_string(),
        Some(Some(group)) => format!("grp = {}", page_args.push(group.clone())),
    };
    let selected_day = match &q.day {
        None => "true".to_string(),
        Some(None) => "day IS NULL".to_string(),
        Some(Some(day)) => format!("day = {}::date", page_args.push(day.to_string())),
    };
    let key_hash = sort_tuple_hash_sql(
        &ordering
            .iter()
            .map(|(alias, _)| alias.clone())
            .collect::<Vec<_>>(),
    );
    let fingerprint = sha256_hex(
        &json!({
            "ws": ws,
            "userId": actor.user_id,
            "id": collection_id,
            "asOf": as_of.to_rfc3339(),
            "timeZone": time_zone,
            "config": q.config.to_json(),
            "window": q.window.as_ref().map(|w| json!([w.from, w.to, w.time_zone])),
            "group": q.group,
            "day": q.day,
            "fields": fields.iter().map(|f| json!([f.id, f.version])).collect::<Vec<_>>(),
            "catalog": compiled.catalog.iter().map(|f| json!([f.id, f.field_type, f.version])).collect::<Vec<_>>(),
        })
        .to_string(),
    );
    let cursor_sql = match &cursor {
        None => "true".to_string(),
        Some(cursor) => {
            if cursor.f != fingerprint {
                return Ok(Err(CollectionDbError::InvalidCursor));
            }
            let id_ph = page_args.push(cursor.id.to_string());
            let key_ph = page_args.push(cursor.key.clone());
            let sql = format!(
                "{source} SELECT EXISTS (SELECT 1 FROM visible WHERE id = {id_ph}::uuid \
                 AND {key_hash} = {key_ph} AND ({selected}) AND ({selected_day}))"
            );
            let found: bool = bind_all(&sql, &page_args.values)
                .fetch_one(&mut **tx)
                .await?
                .try_get(0)?;
            if !found {
                return Ok(Err(CollectionDbError::InvalidCursor));
            }
            after_sort_tuple(
                &ordering,
                |expr| format!("(SELECT {expr} FROM visible WHERE id = {id_ph}::uuid)"),
                "id",
                &id_ph,
            )
        }
    };
    let order_sql = preview_order;
    let sql = format!(
        "{source} SELECT {ROW_COLUMNS}, {key_hash} AS key_hash FROM visible \
         WHERE ({selected}) AND ({cursor_sql}) AND ({selected_day}) \
         ORDER BY {order_sql} LIMIT {}",
        q.limit + 1
    );
    let fetched = bind_all(&sql, &page_args.values)
        .fetch_all(&mut **tx)
        .await?;
    let mut has_more = fetched.len() as i64 > q.limit;
    let mut rows: Vec<(QueryRow, String)> = Vec::new();
    for row in fetched.iter().take(q.limit as usize) {
        rows.push((row_from(row)?, row.try_get("key_hash")?));
    }

    // Response budget: envelope + each row's JSON (values sized in SQL first).
    if !rows.is_empty() {
        let ids: Vec<Uuid> = rows.iter().map(|(row, _)| row.id).collect();
        let sizes: Vec<(Uuid, i64)> = sqlx::query_as(&format!(
            "WITH v AS ({}) SELECT item_id, sum(octet_length(value::text) + 40)::bigint FROM v GROUP BY item_id",
            values_sql("$2")
        ))
        .bind(ws)
        .bind(&ids)
        .fetch_all(&mut **tx)
        .await?;
        let size_of: HashMap<Uuid, i64> = sizes.into_iter().collect();
        let envelope = 4096
            + days.len() * 48
            + previews
                .iter()
                .map(|p| 512 + p.title.len() * 6)
                .sum::<usize>()
            + catalog.iter().map(|c| 128 + c.1.len() * 6).sum::<usize>()
            + rows.len() * 40;
        let mut used = envelope;
        let mut keep = 0;
        for (row, _) in &rows {
            let bytes =
                512 + row.title.len() * 6 + size_of.get(&row.id).copied().unwrap_or(0) as usize;
            if used + bytes > QUERY_RESPONSE_BYTES {
                break;
            }
            used += bytes;
            keep += 1;
        }
        if keep == 0 {
            keep = 1;
        }
        if keep < rows.len() {
            has_more = true;
            rows.truncate(keep);
        }
    }
    let ids: Vec<Uuid> = rows.iter().map(|(row, _)| row.id).collect();
    if !ids.is_empty() {
        let values: Vec<(Uuid, Uuid, Option<Value>)> = sqlx::query_as(&values_sql("$2"))
            .bind(ws)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
        for (item_id, field_id, value) in values {
            if let (Some((row, _)), Some(value)) =
                (rows.iter_mut().find(|(row, _)| row.id == item_id), value)
            {
                row.values.push((field_id, value));
            }
        }
    }

    // Edit rights: project collections share the project level; wiki rows
    // use each document's effective level (workspace base or group grant).
    let writable = !access.archived;
    let can_edit = writable && access.permission.at_least(ProjectPermission::Edit);
    let mut row_edit: HashMap<Uuid, bool> = HashMap::new();
    let docs: HashSet<Uuid> = rows
        .iter()
        .map(|(row, _)| row)
        .chain(previews.iter())
        .filter_map(|row| row.document_id)
        .collect();
    if collection.project_id.is_none() && writable {
        let base = workspace_base_permission(role);
        for doc in docs {
            let level = if base.at_least(ProjectPermission::Edit) {
                base
            } else {
                document_permission(tx, ws, actor.user_id, doc, true).await?
            };
            row_edit.insert(doc, level.at_least(ProjectPermission::Edit));
        }
    }
    let edit_of = |row: &QueryRow| -> bool {
        if collection.project_id.is_some() {
            can_edit
        } else {
            row.document_id
                .and_then(|doc| row_edit.get(&doc).copied())
                .unwrap_or(false)
        }
    };
    for preview in &mut previews {
        preview.can_edit = edit_of(preview);
    }
    let next_cursor = match rows.last() {
        Some((last, key)) if has_more => Some(encode_cursor(&TaskListCursor {
            id: last.id,
            key: key.clone(),
            f: fingerprint,
            as_of,
        })),
        _ => None,
    };
    let items: Vec<QueryRow> = rows
        .into_iter()
        .map(|(mut row, _)| {
            row.can_edit = edit_of(&row);
            row
        })
        .collect();
    let groups = catalog
        .into_iter()
        .map(|(id, name, deleted)| QueryGroup {
            count: buckets
                .iter()
                .find(|(bucket, _)| *bucket == id)
                .map(|(_, count)| *count)
                .unwrap_or(0),
            item_ids: items
                .iter()
                .filter(|item| item.group == id)
                .map(|item| item.id)
                .collect(),
            id,
            name,
            deleted,
        })
        .collect();
    Ok(Ok(QueryResult {
        can_edit,
        days,
        items,
        groups,
        count,
        next_cursor,
        previews,
    }))
}
