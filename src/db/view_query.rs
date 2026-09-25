//! View-query SQL compiler (source `packages/db/src/pg/repos/view-query.ts`).
//!
//! Compiles a parsed [`ViewQuery`] into WHERE conditions and ORDER BY
//! expressions over a root row alias. Every user-supplied value is a bind
//! parameter; the SQL text only ever contains fixed column names, fixed
//! expressions and `$n` placeholders. Referenced catalog rows (fields,
//! options, statuses, labels, milestones, members) are checked in the caller's
//! transaction, and a reference that does not resolve makes the whole query
//! invalid instead of silently matching nothing.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::tasks::list_query::{
    AssigneeFilter, CustomOperator, CustomValue, SortDirection, SortField, ViewQuery, ViewSort,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootKind {
    Task,
    Document,
}

impl RootKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Document => "document",
        }
    }

    fn item_column(self) -> &'static str {
        match self {
            Self::Task => "task_id",
            Self::Document => "document_id",
        }
    }
}

/// Where the query runs. A task query always has a project; a collection
/// query pins its collection so catalog lookups never leave it.
#[derive(Debug, Clone, Copy)]
pub struct ViewScope {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub collection_id: Option<Uuid>,
    pub kind: RootKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidViewQuery;

/// Text bind list; the SQL casts each placeholder to its column type.
#[derive(Debug, Clone)]
pub struct SqlArgs {
    first: usize,
    pub values: Vec<String>,
}

impl SqlArgs {
    /// `first` is the placeholder number of the first pushed value.
    pub fn starting_at(first: usize) -> Self {
        Self {
            first,
            values: Vec::new(),
        }
    }

    pub fn push(&mut self, value: impl Into<String>) -> String {
        self.values.push(value.into());
        format!("${}", self.first + self.values.len() - 1)
    }

    pub fn push_uuid_array(&mut self, ids: &[Uuid]) -> String {
        let literal = format!(
            "{{{}}}",
            ids.iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        format!("{}::uuid[]", self.push(literal))
    }
}

/// One ORDER BY term. `template` contains `{root}` where the root alias goes,
/// so the same term can be evaluated for a cursor anchor row.
#[derive(Debug, Clone)]
pub struct OrderTerm {
    pub field: SortField,
    pub desc: bool,
    template: String,
}

impl OrderTerm {
    pub fn sql(&self, root: &str) -> String {
        self.template.replace("{root}", root)
    }
}

#[derive(Debug, Clone)]
pub struct CatalogField {
    pub id: Uuid,
    pub field_type: String,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct CompiledView {
    /// Conjunctive SQL conditions over `{root}` (already substituted).
    pub conditions: Vec<String>,
    pub order: Vec<OrderTerm>,
    /// Referenced fields in id order; their versions bind cursors to the schema.
    pub catalog: Vec<CatalogField>,
}

impl CompiledView {
    pub fn where_sql(&self) -> String {
        if self.conditions.is_empty() {
            "true".to_string()
        } else {
            format!("({})", self.conditions.join(" AND "))
        }
    }
}

pub const SET_FIELD_TYPES: &[&str] = &[
    "select",
    "multi_select",
    "checkboxes",
    "labels",
    "user",
    "user_multi",
];

pub fn value_column(field_type: &str) -> Option<&'static str> {
    match field_type {
        "text" | "paragraph" => Some("value_text"),
        "number" => Some("value_number"),
        "date" => Some("value_date"),
        "datetime" => Some("value_ts"),
        "checkbox" => Some("value_bool"),
        _ => None,
    }
}

/// Scalar value of `field` for the root row (`NULL` when unset).
pub fn scalar_value_sql(kind: RootKind, root: &str, field_ph: &str, column: &str) -> String {
    format!(
        "(SELECT v.{column} FROM fvoci.collection_items ci \
         JOIN fvoci.collection_values v ON v.workspace_id = ci.workspace_id \
         AND v.collection_id = ci.collection_id AND v.item_id = ci.id AND v.field_id = {field_ph}::uuid \
         WHERE ci.workspace_id = {root}.workspace_id AND ci.{target} = {root}.id)",
        target = kind.item_column()
    )
}

fn set_exists_sql(
    kind: RootKind,
    root: &str,
    field_ph: &str,
    people: bool,
    extra: Option<String>,
) -> String {
    let table = if people {
        "collection_people"
    } else {
        "collection_choices"
    };
    let extra = extra.map(|cond| format!(" AND {cond}")).unwrap_or_default();
    format!(
        "EXISTS (SELECT 1 FROM fvoci.collection_items ci \
         JOIN fvoci.{table} v ON v.workspace_id = ci.workspace_id \
         AND v.collection_id = ci.collection_id AND v.item_id = ci.id AND v.field_id = {field_ph}::uuid \
         WHERE ci.workspace_id = {root}.workspace_id AND ci.{target} = {root}.id{extra})",
        target = kind.item_column()
    )
}

/// Task due date on the calendar of `tz_ph` (source `coalesce(due_date, due_at AT TIME ZONE tz)`).
pub fn due_date_sql(root: &str, tz_ph: &str) -> String {
    format!("COALESCE({root}.due_date, ({root}.due_at AT TIME ZONE {tz_ph})::date)")
}

const TASK_ONLY_SORTS: &[SortField] = &[SortField::Priority, SortField::Due, SortField::Status];

pub struct CompileOptions<'a> {
    pub actor_user_id: Uuid,
    pub time_zone: &'a str,
    /// The task list keeps its own (accepted) standard-filter SQL and asks the
    /// compiler only for `custom`, `dueBefore` and field sorts.
    pub standard_filters: bool,
}

pub async fn compile_view_query(
    tx: &mut Transaction<'_, Postgres>,
    scope: ViewScope,
    query: &ViewQuery,
    options: &CompileOptions<'_>,
    root: &str,
    args: &mut SqlArgs,
) -> Result<Result<CompiledView, InvalidViewQuery>, sqlx::Error> {
    let filters = &query.filters;
    let ws = scope.workspace_id;
    if scope.kind == RootKind::Document {
        let has_task_filter = filters.task_type.is_some()
            || filters.status_id.is_some()
            || filters.assignee_id.is_some()
            || filters.priority.is_some()
            || filters.label_id.is_some()
            || filters.milestone_id.is_some()
            || filters.open_only
            || filters.due_before.is_some();
        if has_task_filter
            || query
                .sort
                .iter()
                .any(|entry| TASK_ONLY_SORTS.contains(&entry.field))
        {
            return Ok(Err(InvalidViewQuery));
        }
    }
    if scope.project_id.is_none() && scope.collection_id.is_none() {
        return Ok(Err(InvalidViewQuery));
    }

    let mut field_ids: Vec<Uuid> = filters.custom.iter().map(|item| item.field_id).collect();
    for entry in &query.sort {
        if let SortField::Field(id) = entry.field {
            field_ids.push(id);
        }
    }
    field_ids.sort();
    field_ids.dedup();
    let catalog = load_catalog(tx, scope, &field_ids).await?;
    if catalog.len() != field_ids.len() {
        return Ok(Err(InvalidViewQuery));
    }
    let field_type = |id: Uuid| -> &str {
        catalog
            .iter()
            .find(|field| field.id == id)
            .map(|field| field.field_type.as_str())
            .unwrap_or("")
    };

    let mut conditions: Vec<String> = Vec::new();
    // Each check is `SELECT <bool>`; all must be true.
    let mut checks: Vec<String> = Vec::new();
    let mut check_args = SqlArgs::starting_at(1);
    let project_ph_check = scope.project_id.map(|id| check_args.push(id.to_string()));
    let ws_ph_check = check_args.push(ws.to_string());

    if options.standard_filters {
        if let Some(task_type) = &filters.task_type {
            conditions.push(format!("{root}.type = {}", args.push(task_type.clone())));
        }
        if let Some(priority) = &filters.priority {
            conditions.push(format!("{root}.priority = {}", args.push(priority.clone())));
        }
        if let Some(status_id) = filters.status_id {
            let Some(project_ph) = &project_ph_check else {
                return Ok(Err(InvalidViewQuery));
            };
            let status = check_args.push(status_id.to_string());
            checks.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.statuses s WHERE s.workspace_id = {ws_ph_check}::uuid \
                 AND s.project_id = {project_ph}::uuid AND s.id = {status}::uuid)"
            ));
            conditions.push(format!(
                "{root}.status_id = {}::uuid",
                args.push(status_id.to_string())
            ));
        }
        if let Some(milestone_id) = filters.milestone_id {
            let Some(project_ph) = &project_ph_check else {
                return Ok(Err(InvalidViewQuery));
            };
            let milestone = check_args.push(milestone_id.to_string());
            checks.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.milestones m WHERE m.workspace_id = {ws_ph_check}::uuid \
                 AND m.project_id = {project_ph}::uuid AND m.id = {milestone}::uuid)"
            ));
            conditions.push(format!(
                "{root}.milestone_id = {}::uuid",
                args.push(milestone_id.to_string())
            ));
        }
        if let Some(label_id) = filters.label_id {
            let Some(project_ph) = &project_ph_check else {
                return Ok(Err(InvalidViewQuery));
            };
            let label = check_args.push(label_id.to_string());
            checks.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.labels l WHERE l.workspace_id = {ws_ph_check}::uuid \
                 AND l.project_id = {project_ph}::uuid AND l.id = {label}::uuid)"
            ));
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.task_labels tl WHERE tl.workspace_id = {root}.workspace_id \
                 AND tl.task_id = {root}.id AND tl.label_id = {}::uuid)",
                args.push(label_id.to_string())
            ));
        }
        if let Some(assignee) = &filters.assignee_id {
            let user_id = match assignee {
                AssigneeFilter::Me => options.actor_user_id,
                AssigneeFilter::User(id) => {
                    let user = check_args.push(id.to_string());
                    checks.push(live_member_check_sql(&ws_ph_check, &user));
                    *id
                }
            };
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.task_assignees ta WHERE ta.workspace_id = {root}.workspace_id \
                 AND ta.task_id = {root}.id AND ta.user_id = {}::uuid)",
                args.push(user_id.to_string())
            ));
        }
        if filters.open_only {
            conditions.push(format!(
                "EXISTS (SELECT 1 FROM fvoci.statuses so WHERE so.workspace_id = {root}.workspace_id \
                 AND so.id = {root}.status_id AND so.category NOT IN ('done', 'canceled'))"
            ));
        }
        if let Some(title) = &filters.title {
            conditions.push(format!(
                "strpos(lower({root}.title), lower({})) > 0",
                args.push(title.clone())
            ));
        }
    }
    if let Some(due_before) = filters.due_before {
        let tz = args.push(options.time_zone.to_string());
        conditions.push(format!(
            "{} <= {}::date",
            due_date_sql(root, &tz),
            args.push(due_before.to_string())
        ));
    }

    for item in &filters.custom {
        let kind_of = field_type(item.field_id).to_string();
        let field_ph = args.push(item.field_id.to_string());
        if SET_FIELD_TYPES.contains(&kind_of.as_str()) {
            let people = kind_of == "user" || kind_of == "user_multi";
            match &item.operator {
                CustomOperator::Empty => {
                    conditions.push(format!(
                        "NOT {}",
                        set_exists_sql(scope.kind, root, &field_ph, people, None)
                    ));
                }
                CustomOperator::Equals(CustomValue::Text(raw)) => {
                    let Some(value) = parse_uuid_text(raw) else {
                        return Ok(Err(InvalidViewQuery));
                    };
                    let value_ph_check = check_args.push(value.to_string());
                    if people {
                        checks.push(live_member_check_sql(&ws_ph_check, &value_ph_check));
                    } else {
                        let field_ph_check = check_args.push(item.field_id.to_string());
                        checks.push(format!(
                            "EXISTS (SELECT 1 FROM fvoci.collection_options o \
                             WHERE o.workspace_id = {ws_ph_check}::uuid AND o.field_id = {field_ph_check}::uuid \
                             AND o.id = {value_ph_check}::uuid AND o.deleted_at IS NULL)"
                        ));
                    }
                    let column = if people { "user_id" } else { "option_id" };
                    let value_ph = args.push(value.to_string());
                    conditions.push(set_exists_sql(
                        scope.kind,
                        root,
                        &field_ph,
                        people,
                        Some(format!("v.{column} = {value_ph}::uuid")),
                    ));
                }
                CustomOperator::Equals(_) => return Ok(Err(InvalidViewQuery)),
            }
            continue;
        }
        let Some(column) = value_column(&kind_of) else {
            return Ok(Err(InvalidViewQuery));
        };
        let expression = scalar_value_sql(scope.kind, root, &field_ph, column);
        let condition = match (&item.operator, kind_of.as_str()) {
            (CustomOperator::Empty, _) => format!("{expression} IS NULL"),
            (CustomOperator::Equals(CustomValue::Number(number)), "number") => {
                format!(
                    "{expression} = {}::numeric",
                    args.push(format_number(*number))
                )
            }
            (CustomOperator::Equals(CustomValue::Bool(flag)), "checkbox") => {
                format!("{expression} = {}::boolean", args.push(flag.to_string()))
            }
            (CustomOperator::Equals(CustomValue::Text(raw)), "date") => {
                let Some(date) = crate::tasks::parse_iso_date(raw) else {
                    return Ok(Err(InvalidViewQuery));
                };
                format!("{expression} = {}::date", args.push(date.to_string()))
            }
            (CustomOperator::Equals(CustomValue::Text(raw)), "datetime") => {
                let Some(at) = crate::tasks::parse_iso_datetime(raw) else {
                    return Ok(Err(InvalidViewQuery));
                };
                format!("{expression} = {}::timestamptz", args.push(at.to_rfc3339()))
            }
            (CustomOperator::Equals(CustomValue::Text(raw)), "text" | "paragraph") => {
                format!("{expression} = {}", args.push(raw.clone()))
            }
            _ => return Ok(Err(InvalidViewQuery)),
        };
        conditions.push(condition);
    }

    if !checks.is_empty() {
        let sql = format!("SELECT {}", checks.join(" AND "));
        let mut check = sqlx::query_scalar::<_, bool>(&sql);
        for value in &check_args.values {
            check = check.bind(value);
        }
        if !check.fetch_one(&mut **tx).await? {
            return Ok(Err(InvalidViewQuery));
        }
    }

    let default_sort = ViewSort {
        field: if scope.kind == RootKind::Task {
            SortField::Rank
        } else {
            SortField::Created
        },
        direction: SortDirection::Asc,
    };
    let sort: Vec<ViewSort> = if query.sort.is_empty() {
        vec![default_sort]
    } else {
        query.sort.clone()
    };
    let mut order = Vec::with_capacity(sort.len());
    for entry in sort {
        let template = match entry.field {
            SortField::Field(id) => {
                let kind_of = field_type(id);
                let Some(column) = value_column(kind_of) else {
                    return Ok(Err(InvalidViewQuery));
                };
                let field_ph = args.push(id.to_string());
                scalar_value_sql(scope.kind, "{root}", &field_ph, column)
            }
            SortField::Title => r#"{root}.title COLLATE "C""#.to_string(),
            SortField::Number => "{root}.number".to_string(),
            SortField::Rank => r#"{root}.sort_key COLLATE "C""#.to_string(),
            SortField::Created => "{root}.created_at".to_string(),
            SortField::Updated => "{root}.updated_at".to_string(),
            SortField::Priority => "CASE {root}.priority WHEN 'none' THEN 0 WHEN 'low' THEN 1 \
                 WHEN 'medium' THEN 2 WHEN 'high' THEN 3 WHEN 'urgent' THEN 4 END"
                .to_string(),
            SortField::Due => {
                let tz = args.push(options.time_zone.to_string());
                due_date_sql("{root}", &tz)
            }
            SortField::Status => "(SELECT st.sort_key COLLATE \"C\" FROM fvoci.statuses st \
                 WHERE st.workspace_id = {root}.workspace_id AND st.id = {root}.status_id)"
                .to_string(),
        };
        order.push(OrderTerm {
            field: entry.field,
            desc: entry.direction == SortDirection::Desc,
            template,
        });
    }

    let conditions = conditions
        .into_iter()
        .map(|condition| condition.replace("{root}", root))
        .collect();
    Ok(Ok(CompiledView {
        conditions,
        order,
        catalog,
    }))
}

fn live_member_check_sql(ws_ph: &str, user_ph: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM fvoci.memberships m JOIN fvoci.users u ON u.id = m.user_id \
         WHERE m.workspace_id = {ws_ph}::uuid AND u.deleted_at IS NULL AND u.id = {user_ph}::uuid)"
    )
}

fn parse_uuid_text(raw: &str) -> Option<Uuid> {
    if raw.len() != 36 {
        return None;
    }
    Uuid::parse_str(raw).ok()
}

/// Decimal text PostgreSQL `numeric` accepts; Rust `f64` Display never uses exponents.
pub fn format_number(value: f64) -> String {
    format!("{value}")
}

async fn load_catalog(
    tx: &mut Transaction<'_, Postgres>,
    scope: ViewScope,
    ids: &[Uuid],
) -> Result<Vec<CatalogField>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, String, i32)> = sqlx::query_as(
        r#"
        SELECT f.id, f.type, f.version
        FROM fvoci.collection_fields f
        JOIN fvoci.collections c ON c.workspace_id = f.workspace_id AND c.id = f.collection_id
        WHERE f.workspace_id = $1
          AND f.deleted_at IS NULL
          AND c.deleted_at IS NULL
          AND c.kind = $2
          AND f.id = ANY($3)
          AND ($4::uuid IS NULL OR c.id = $4)
          AND ($5::uuid IS NULL OR c.project_id = $5)
        ORDER BY f.id
        "#,
    )
    .bind(scope.workspace_id)
    .bind(scope.kind.as_str())
    .bind(ids)
    .bind(scope.collection_id)
    .bind(scope.project_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, field_type, version)| CatalogField {
            id,
            field_type,
            version,
        })
        .collect())
}

/// Keyset condition "row sorts strictly after the anchor": NULLS LAST in both
/// directions, `id ASC` as the final unique tie-breaker (source `afterSortTuple`).
/// `anchor` maps an expression to the anchor row's value of it.
pub fn after_sort_tuple(
    order: &[(String, bool)],
    anchor: impl Fn(&str) -> String,
    id_sql: &str,
    anchor_id_ph: &str,
) -> String {
    let mut equal: Vec<String> = Vec::new();
    let mut branches: Vec<String> = Vec::new();
    for (expression, desc) in order {
        let a = anchor(expression);
        let op = if *desc { "<" } else { ">" };
        let mut parts = equal.clone();
        parts.push(format!(
            "({expression} IS NULL AND {a} IS NOT NULL OR {expression} {op} {a})"
        ));
        branches.push(format!("({})", parts.join(" AND ")));
        equal.push(format!("{expression} IS NOT DISTINCT FROM {a}"));
    }
    let mut last = equal;
    last.push(format!("{id_sql} > {anchor_id_ph}::uuid"));
    branches.push(format!("({})", last.join(" AND ")));
    format!("({})", branches.join(" OR "))
}

/// SHA-256 over the JSON array of sort values: binds a cursor to its anchor's
/// sort tuple without echoing the values (source `sortTupleHash`).
pub fn sort_tuple_hash_sql(expressions: &[String]) -> String {
    format!(
        "encode(sha256(convert_to(jsonb_build_array({})::text, 'UTF8')), 'hex')",
        expressions.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_number_placeholders_from_offset() {
        let mut args = SqlArgs::starting_at(3);
        assert_eq!(args.push("a"), "$3");
        assert_eq!(args.push("b"), "$4");
        assert_eq!(args.push_uuid_array(&[Uuid::nil()]), "$5::uuid[]");
        assert_eq!(args.values[2], "{00000000-0000-0000-0000-000000000000}");
    }

    #[test]
    fn after_tuple_orders_nulls_last_and_ties_by_id() {
        let sql = after_sort_tuple(
            &[("x".to_string(), false), ("y".to_string(), true)],
            |e| format!("A({e})"),
            "id",
            "$9",
        );
        assert!(sql.contains("(x IS NULL AND A(x) IS NOT NULL OR x > A(x))"));
        assert!(sql.contains(
            "x IS NOT DISTINCT FROM A(x) AND (y IS NULL AND A(y) IS NOT NULL OR y < A(y))"
        ));
        assert!(sql.ends_with("y IS NOT DISTINCT FROM A(y) AND id > $9::uuid))"));
    }

    #[test]
    fn number_text_is_plain_decimal() {
        assert_eq!(format_number(1e21), "1000000000000000000000");
        assert_eq!(format_number(-0.5), "-0.5");
    }
}
