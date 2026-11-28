//! Shared course schedules (`course_schedules`), shared/personal redistribution,
//! personal planned-date overrides, and workspace today/overdue queries.
//!
//! Shared schedule rows survive soft deletion of a course into the trash and are removed
//! on permanent purge ([`forget_gone_containers`]). Every personal planned-date read and
//! write is strictly scoped to the calling user (`user_id`) and the handle's
//! organization.

use std::collections::{BTreeMap, HashMap, HashSet};

use fvoci_model::{
    Block, BlockType, ContainerKind, ContainerKinds, CourseOutlineNodes, CourseSchedule, DiaryDate,
    JsonMap, RedistributeScheduleRequest, StudyDays, UNIT_SCHEDULED_DATE_FIELD, UnitNode,
    UnitProgress, UnitWeight, ValidatedCourseSchedule, WorkspaceLearningToday,
    build_workspace_learning_today, distribute_units, is_id, outline_units,
};
use serde_json::Value;

use crate::Store;
use crate::containers::{fetch_containers, lock_container_of_kind, select_containers_of_user};
use crate::db::{Dialect, FromRow, Id, LockRank, OptId, Param, Row, RowLock, Scope};
use crate::error::StoreError;
use crate::unit_progress::require_live_unit;
use fvoci_model::PublicContainers;

/// Maximum rows written per batched statement during schedule redistribution.
const REDISTRIBUTE_BATCH_SIZE: usize = 100;

/// Metadata-only unit/chapter row paired with its decoded `fields` map (never reads `body`).
struct NodeRow {
    node: UnitNode,
    fields: JsonMap,
}

impl Store {
    /// Returns the saved shared schedule for live course `course_id`, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::Corrupt`] if stored `study_days` is outside `1..=127`, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn get_course_schedule(
        &self,
        course_id: &str,
    ) -> Result<Option<CourseSchedule>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            fetch_course_schedule(scope, course_id, org).await
        })
        .await
    }

    /// Creates or updates the shared schedule of live course `course_id`. When
    /// `schedule.auto_assign` is `true`, proportionally assigns every live unit's shared
    /// `scheduledDate` in the same transaction and returns the IDs of modified units.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course in a live workspace
    /// and organization, and [`StoreError::Database`] if a statement fails.
    pub async fn upsert_course_schedule(
        &self,
        course_id: &str,
        schedule: &ValidatedCourseSchedule,
        user_id: &str,
    ) -> Result<(CourseSchedule, Vec<String>), StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course_locked(scope, course_id, org.clone()).await?;
            let saved = upsert_schedule_row(scope, course_id, schedule, org).await?;
            let updated_ids = if schedule.auto_assign {
                assign_shared_unit_dates(
                    scope,
                    course_id,
                    schedule.start_date,
                    schedule.target_date,
                    schedule.study_days,
                    user_id,
                )
                .await?
            } else {
                Vec::new()
            };
            Ok((saved, updated_ids))
        })
        .await
    }

    /// Deletes the saved shared schedule of live course `course_id` (idempotent when no
    /// schedule row exists). Does not alter unit `scheduledDate` fields or personal
    /// progress rows.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn delete_course_schedule(&self, course_id: &str) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course_locked(scope, course_id, org.clone()).await?;
            let mut sql = scope.sql("DELETE FROM course_schedules WHERE container_id = ");
            sql.changing_no_access()
                .bind(Id(course_id))
                .push_bind(" AND org_id = ", org);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Manually redistributes the shared `scheduledDate` of all live units in `course_id`,
    /// resolving any omitted request fields against the course's saved schedule inside
    /// the write transaction, and returns the IDs of modified units. Never reads any
    /// user's personal progress.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course,
    /// [`StoreError::Refused`] if the resolved redistribution window is missing or
    /// invalid, and [`StoreError::Database`] if a statement fails.
    pub async fn redistribute_course_schedule(
        &self,
        course_id: &str,
        request: &RedistributeScheduleRequest,
        today: DiaryDate,
        user_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course_locked(scope, course_id, org.clone()).await?;
            let saved = fetch_course_schedule(scope, course_id, org).await?;
            let plan = request.resolve(today, saved.as_ref())?;
            assign_shared_unit_dates(
                scope,
                course_id,
                plan.from_date,
                plan.target_date,
                plan.study_days,
                user_id,
            )
            .await
        })
        .await
    }

    /// Sets or clears (`None`) the calling learner's personal `planned_date` on a live
    /// unit of `course_id`, preserving `completed_at` and `last_position`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `unit_id` is not a live unit of live course
    /// `course_id` or `user_id` is not an active member of its workspace, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn set_unit_planned_date(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        planned_date: Option<DiaryDate>,
    ) -> Result<UnitProgress, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_unit(scope, course_id, unit_id, user_id, org.clone()).await?;
            let now = scope.now_millis();
            upsert_planned_date_row(
                scope,
                course_id,
                unit_id,
                user_id,
                org,
                planned_date.map(|d| d.to_string()),
                now,
            )
            .await
        })
        .await
    }

    /// Manually redistributes only the calling learner's incomplete live units in
    /// `course_id`, resolving any omitted request fields against the course's saved
    /// schedule inside the same transaction and updating only the caller's
    /// `unit_progress.planned_date` rows. Completed units (including those completed
    /// concurrently) and other learners' rows are never modified.
    ///
    /// Locks are acquired in the global lock order (`containers` -> `blocks` ->
    /// `workspace_members` -> `users` -> `unit_progress` ordered by `unit_id`) to prevent
    /// deadlocks and orphan writes on PostgreSQL.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course or `user_id` is not
    /// an active member of its workspace, [`StoreError::Refused`] if the resolved
    /// redistribution window is missing or invalid, and [`StoreError::Database`] if a
    /// statement fails.
    pub async fn redistribute_personal_schedule(
        &self,
        course_id: &str,
        user_id: &str,
        request: &RedistributeScheduleRequest,
        today: DiaryDate,
    ) -> Result<(), StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            let workspace_id = lock_live_course_for_learner(scope, course_id, org.clone()).await?;
            let nodes = fetch_course_nodes_locked(scope, course_id).await?;
            lock_active_learner(scope, course_id, &workspace_id, user_id, org.clone()).await?;

            let saved = fetch_course_schedule(scope, course_id, org.clone()).await?;
            let plan = request.resolve(today, saved.as_ref())?;

            let completed =
                fetch_completed_unit_ids(scope, course_id, user_id, org.clone()).await?;
            let incomplete: Vec<&UnitNode> = outline_units(&nodes)
                .into_iter()
                .filter(|unit| !completed.contains(unit.id.as_str()))
                .collect();
            let weights: Vec<UnitWeight<'_>> = incomplete
                .iter()
                .map(|unit| UnitWeight {
                    unit_id: unit.id.as_str(),
                    amount: unit.amount,
                })
                .collect();
            let mut assignments =
                distribute_units(&weights, plan.from_date, plan.target_date, plan.study_days);
            assignments.sort_by(|a, b| a.0.cmp(&b.0));

            let now = scope.now_millis();
            upsert_personal_planned_dates_if_incomplete(
                scope,
                course_id,
                user_id,
                org,
                &assignments,
                now,
            )
            .await?;
            Ok(())
        })
        .await
    }

    /// Returns the calling learner's incomplete units due `today` and overdue across all
    /// live courses in `workspace_id` where `user_id` is enrolled (direct course member
    /// or workspace worker with an inherited group role).
    ///
    /// Never reads another user's `unit_progress` rows and never exposes private courses
    /// the caller is not enrolled in.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if a query fails.
    pub async fn get_workspace_learning_today(
        &self,
        workspace_id: &str,
        user_id: &str,
        today: DiaryDate,
    ) -> Result<WorkspaceLearningToday, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let courses = fetch_enrolled_courses(scope, workspace_id, user_id, org.clone()).await?;
            if courses.is_empty() {
                return Ok(WorkspaceLearningToday {
                    today: today.to_string(),
                    today_units: Vec::new(),
                    overdue_units: Vec::new(),
                });
            }
            let course_ids: Vec<&str> = courses.iter().map(|(id, _)| id.as_str()).collect();
            let mut nodes_by_course = fetch_nodes_for_courses(scope, &course_ids).await?;
            let progress = fetch_my_progress_for_courses(scope, &course_ids, user_id, org).await?;
            let assembled: Vec<CourseOutlineNodes> = courses
                .into_iter()
                .map(|(course_id, course_title)| {
                    let nodes = nodes_by_course.remove(&course_id).unwrap_or_default();
                    CourseOutlineNodes {
                        course_id,
                        course_title,
                        nodes,
                    }
                })
                .collect();
            Ok(build_workspace_learning_today(&assembled, &progress, today))
        })
        .await
    }
}

pub(crate) async fn fetch_course_schedule(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: Param,
) -> Result<Option<CourseSchedule>, StoreError> {
    let mut sql = scope.sql(
        "SELECT cs.start_date, cs.target_date, CAST(cs.study_days AS BIGINT), \
         cs.updated_at \
         FROM course_schedules AS cs \
         JOIN containers AS c ON c.id = cs.container_id \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE cs.container_id = ",
    );
    sql.bind(Id(course_id))
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND cs.org_id = w.org_id")
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("cs", "c"), ("c", "w")]));
    scope.first(sql).await
}

/// Verifies `course_id` is a live course in a live workspace and organization while
/// holding an exclusive row lock on the course container.
async fn require_live_course_locked(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: Param,
) -> Result<(), StoreError> {
    if lock_container_of_kind(scope, course_id).await? != Some(ContainerKind::Course) {
        return Err(StoreError::not_found("course"));
    }
    let mut sql = scope.sql(
        "SELECT 1 FROM containers AS c \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE c.id = ",
    );
    sql.bind(Id(course_id))
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("c", "w")]));
    if !scope.exists(sql).await? {
        return Err(StoreError::not_found("course"));
    }
    Ok(())
}

/// Locks the live course container (`FOR KEY SHARE` on PostgreSQL) and returns its
/// workspace identifier.
async fn lock_live_course_for_learner(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: Param,
) -> Result<String, StoreError> {
    let mut sql = scope.sql(
        "SELECT c.workspace_id \
         FROM containers AS c \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE c.id = ",
    );
    sql.bind(Id(course_id))
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("c", "w")]));
    sql.push_lock(scope.row_lock(LockRank::Container, course_id, RowLock::KeyShare.of("c")));
    scope.one(sql, "course").await
}

/// Locks `workspace_members` (`FOR SHARE`) and `users` (`FOR KEY SHARE`), ensuring
/// `user_id` is an active enrolled member of `course_id` in `workspace_id`.
async fn lock_active_learner(
    scope: &mut Scope<'_>,
    course_id: &str,
    workspace_id: &str,
    user_id: &str,
    org: Param,
) -> Result<(), StoreError> {
    let mut member = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    member
        .bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    member.push_lock(scope.row_lock(
        LockRank::WorkspaceMembers,
        &format!("{workspace_id}:{user_id}"),
        RowLock::Share,
    ));
    if !scope.exists(member).await? {
        return Err(StoreError::not_found("course"));
    }

    let mut user = scope.sql("SELECT 1 FROM users WHERE id = ");
    user.bind(Id(user_id))
        .push(" AND deleted_at IS NULL")
        .push_bind(" AND erased_at = ", 0_i64);
    user.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::KeyShare));
    if !scope.exists(user).await? {
        return Err(StoreError::not_found("course"));
    }
    crate::course_meetings::require_active_course_member(scope, course_id, user_id, org)
        .await
        .map_err(|error| {
            if error.is_not_found() {
                StoreError::not_found("course")
            } else {
                error
            }
        })
}

pub(crate) async fn upsert_schedule_row(
    scope: &mut Scope<'_>,
    course_id: &str,
    schedule: &ValidatedCourseSchedule,
    org: Param,
) -> Result<CourseSchedule, StoreError> {
    let now = scope.now_millis();
    let start = schedule.start_date.to_string();
    let target = schedule.target_date.to_string();
    let bits = i64::from(schedule.study_days.bits());
    let mut sql = scope.sql(
        "INSERT INTO course_schedules \
         (container_id, start_date, target_date, study_days, updated_at, org_id) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(course_id))
        .push_bind(", ", &start)
        .push_bind(", ", &target);
    if scope.dialect() == Dialect::Postgres {
        sql.push_bind(", CAST(", bits).push("::bigint AS SMALLINT)");
    } else {
        sql.push_bind(", ", bits);
    }
    sql.push_bind(", ", now).push_bind(", ", org).push(
        ") ON CONFLICT (container_id, org_id) DO UPDATE SET \
             start_date = excluded.start_date, \
             target_date = excluded.target_date, \
             study_days = excluded.study_days, \
             updated_at = excluded.updated_at \
             RETURNING start_date, target_date, CAST(study_days AS BIGINT), updated_at",
    );
    scope.first(sql).await?.ok_or(StoreError::NoRows)
}

/// Assigns shared `scheduledDate` values on live units of `course_id` in outline order
/// under the caller's container lock, reading only block metadata (never `body`), and
/// returns the identifiers of modified unit blocks.
async fn assign_shared_unit_dates(
    scope: &mut Scope<'_>,
    course_id: &str,
    start: DiaryDate,
    target: DiaryDate,
    study_days: StudyDays,
    user_id: &str,
) -> Result<Vec<String>, StoreError> {
    let node_rows = fetch_course_node_rows(scope, course_id).await?;
    let nodes: Vec<UnitNode> = node_rows.iter().map(|r| r.node.clone()).collect();
    let ordered = outline_units(&nodes);
    if ordered.is_empty() {
        return Ok(Vec::new());
    }
    let weights: Vec<UnitWeight<'_>> = ordered
        .iter()
        .map(|unit| UnitWeight {
            unit_id: unit.id.as_str(),
            amount: unit.amount,
        })
        .collect();
    let assignments = distribute_units(&weights, start, target, study_days);
    let mut rows_by_id: HashMap<String, NodeRow> = node_rows
        .into_iter()
        .map(|r| (r.node.id.clone(), r))
        .collect();

    let now = scope.now_millis();
    let mut updates: Vec<(String, String, i64)> = Vec::new();
    for (unit_id, date) in assignments {
        let Some(mut row) = rows_by_id.remove(&unit_id) else {
            continue;
        };
        let new_date = date.to_string();
        let same = row
            .fields
            .get(UNIT_SCHEDULED_DATE_FIELD)
            .and_then(Value::as_str)
            == Some(new_date.as_str());
        if same {
            continue;
        }
        row.fields.insert(
            UNIT_SCHEDULED_DATE_FIELD.to_owned(),
            Value::from(new_date.as_str()),
        );
        let check = Block {
            id: row.node.id.clone(),
            container_id: course_id.to_owned(),
            parent_id: row.node.parent_id,
            block_type: BlockType::Unit,
            title: row.node.title,
            fields: row.fields,
            ..Block::default()
        };
        check
            .validate()
            .map_err(|source| StoreError::InvalidBlock {
                id: check.id.clone(),
                source,
            })?;
        let updated_at = now.max(row.node.updated_at.saturating_add(1));
        updates.push((check.id, new_date, updated_at));
    }

    for batch in updates.chunks(REDISTRIBUTE_BATCH_SIZE) {
        execute_scheduled_date_batch(scope, course_id, user_id, batch).await?;
    }

    Ok(updates.into_iter().map(|(id, _, _)| id).collect())
}

async fn execute_scheduled_date_batch(
    scope: &mut Scope<'_>,
    course_id: &str,
    user_id: &str,
    batch: &[(String, String, i64)],
) -> Result<(), StoreError> {
    let mut sql = match scope.dialect() {
        Dialect::Sqlite => scope.sql(
            "UPDATE blocks SET fields = json_set(COALESCE(fields, '{}'), '$.scheduledDate', CASE id",
        ),
        Dialect::Postgres => scope.sql(
            "UPDATE blocks SET fields = jsonb_set(COALESCE(fields::jsonb, '{}'::jsonb), '{scheduledDate}', to_jsonb((CASE id",
        ),
    };
    sql.changing_no_access();
    for (id, new_date, _) in batch {
        sql.push_bind(" WHEN ", Id(id))
            .push_bind(" THEN ", new_date.as_str());
    }
    match scope.dialect() {
        Dialect::Sqlite => sql.push(" END)"),
        Dialect::Postgres => sql.push(" END)::text), true)::json"),
    };
    sql.push_bind(", modified_by = ", OptId(user_id))
        .push(", updated_at = CASE id");
    for (id, _, updated_at) in batch {
        sql.push_bind(" WHEN ", Id(id))
            .push_bind(" THEN ", *updated_at);
    }
    sql.push(" ELSE updated_at END WHERE container_id = ")
        .bind(Id(course_id))
        .push(" AND id IN (")
        .bind_list(batch.iter().map(|(id, _, _)| Id(id)))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

async fn upsert_planned_date_row(
    scope: &mut Scope<'_>,
    course_id: &str,
    unit_id: &str,
    user_id: &str,
    org: Param,
    planned_date: Option<String>,
    now: i64,
) -> Result<UnitProgress, StoreError> {
    let (conflict_target, updated_expr, org_guard) = match scope.dialect() {
        Dialect::Postgres => (
            "(unit_id, org_id, user_id)",
            "GREATEST(unit_progress.updated_at, excluded.updated_at)",
            "",
        ),
        Dialect::Sqlite => (
            "(unit_id, user_id)",
            "MAX(unit_progress.updated_at, excluded.updated_at)",
            " WHERE unit_progress.org_id = excluded.org_id",
        ),
    };
    let mut sql = scope.sql(
        "INSERT INTO unit_progress \
         (unit_id, container_id, user_id, completed_at, planned_date, last_position, \
          updated_at, org_id) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(unit_id))
        .push_bind(", ", Id(course_id))
        .push_bind(", ", Id(user_id))
        .push_bind(", ", Param::OptInt(None))
        .push_bind(", ", Param::OptText(planned_date))
        .push_bind(", ", Param::OptText(None))
        .push_bind(", ", now)
        .push_bind(", ", org)
        .push(") ON CONFLICT ")
        .push(conflict_target)
        .push(
            " DO UPDATE SET container_id = excluded.container_id, \
             planned_date = excluded.planned_date, \
             updated_at = ",
        )
        .push(updated_expr)
        .push(org_guard)
        .push(" RETURNING unit_id, completed_at, planned_date, last_position, updated_at");
    scope.first(sql).await?.ok_or(StoreError::NoRows)
}

/// Inserts or updates personal `planned_date` rows in batches during personal
/// redistribution, leaving rows that were concurrently marked completed untouched
/// (`WHERE completed_at IS NULL`).
async fn upsert_personal_planned_dates_if_incomplete(
    scope: &mut Scope<'_>,
    course_id: &str,
    user_id: &str,
    org: Param,
    assignments: &[(String, DiaryDate)],
    now: i64,
) -> Result<(), StoreError> {
    if assignments.is_empty() {
        return Ok(());
    }
    let (conflict_target, updated_expr) = match scope.dialect() {
        Dialect::Postgres => (
            "(unit_id, org_id, user_id)",
            "GREATEST(unit_progress.updated_at, excluded.updated_at)",
        ),
        Dialect::Sqlite => (
            "(unit_id, user_id)",
            "MAX(unit_progress.updated_at, excluded.updated_at)",
        ),
    };
    for batch in assignments.chunks(REDISTRIBUTE_BATCH_SIZE) {
        let mut sql = scope.sql(
            "INSERT INTO unit_progress \
             (unit_id, container_id, user_id, completed_at, planned_date, last_position, \
              updated_at, org_id) VALUES ",
        );
        sql.changing_no_access();
        for (idx, (unit_id, date)) in batch.iter().enumerate() {
            if idx > 0 {
                sql.push(", ");
            }
            sql.push("(")
                .bind(Id(unit_id))
                .push_bind(", ", Id(course_id))
                .push_bind(", ", Id(user_id))
                .push_bind(", ", Param::OptInt(None))
                .push_bind(", ", date.to_string())
                .push_bind(", ", Param::OptText(None))
                .push_bind(", ", now)
                .push_bind(", ", org.clone())
                .push(")");
        }
        sql.push(" ON CONFLICT ")
            .push(conflict_target)
            .push(
                " DO UPDATE SET container_id = excluded.container_id, \
                 planned_date = excluded.planned_date, \
                 updated_at = ",
            )
            .push(updated_expr)
            .push(
                " WHERE unit_progress.completed_at IS NULL \
                 AND unit_progress.org_id = excluded.org_id",
            );
        scope.execute(sql).await?;
    }
    Ok(())
}

async fn fetch_course_node_rows(
    scope: &mut Scope<'_>,
    course_id: &str,
) -> Result<Vec<NodeRow>, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, parent_id, type, title, fields, created_at, updated_at FROM blocks \
         WHERE container_id = ",
    );
    sql.bind(Id(course_id))
        .push(" AND type IN (")
        .bind_list([BlockType::Unit.as_str(), BlockType::Chapter.as_str()])
        .push(")");
    scope.all(sql).await
}

/// Fetches live units and chapters of `course_id`, locking them in deterministic `id`
/// order (`FOR KEY SHARE` on PostgreSQL) so concurrent deletions/purges cannot race
/// against personal `unit_progress` writes.
async fn fetch_course_nodes_locked(
    scope: &mut Scope<'_>,
    course_id: &str,
) -> Result<Vec<UnitNode>, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, parent_id, type, title, fields, created_at, updated_at FROM blocks \
         WHERE container_id = ",
    );
    sql.bind(Id(course_id))
        .push(" AND type IN (")
        .bind_list([BlockType::Unit.as_str(), BlockType::Chapter.as_str()])
        .push(") ORDER BY id");
    sql.push_lock(scope.row_lock(LockRank::Block, course_id, RowLock::KeyShare));
    let rows = scope.fetch_all(sql).await?;
    rows.iter()
        .map(|row| Ok(NodeRow::from_row(row)?.node))
        .collect()
}

async fn fetch_completed_unit_ids(
    scope: &mut Scope<'_>,
    course_id: &str,
    user_id: &str,
    org: Param,
) -> Result<HashSet<String>, StoreError> {
    let mut sql = scope.sql("SELECT unit_id FROM unit_progress WHERE container_id = ");
    sql.bind(Id(course_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push_bind(" AND org_id = ", org)
        .push(" AND completed_at IS NOT NULL");
    scope.all(sql).await
}

/// Fetches `(course_id, course_title)` for live courses in `workspace_id` where `user_id`
/// holds an enrolled membership (direct role or inherited group role), oldest first.
async fn fetch_enrolled_courses(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    org: Param,
) -> Result<Vec<(String, String)>, StoreError> {
    let mut sql = select_containers_of_user(
        scope,
        user_id,
        PublicContainers::Exclude,
        ContainerKinds::COURSES,
    );
    sql.push_bind(" AND b.workspace_id = ", Id(workspace_id))
        .push_bind(
            " AND EXISTS (SELECT 1 FROM workspaces AS w \
             JOIN organizations AS o ON o.id = w.org_id \
             WHERE w.id = b.workspace_id AND w.org_id = ",
            org,
        )
        .push(" AND w.deleted_at IS NULL AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("w", "b")]))
        .push(") ORDER BY b.created_at, b.id");
    let containers = fetch_containers(scope, sql).await?;
    Ok(containers.into_iter().map(|c| (c.id, c.title)).collect())
}

async fn fetch_nodes_for_courses(
    scope: &mut Scope<'_>,
    course_ids: &[&str],
) -> Result<HashMap<String, Vec<UnitNode>>, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, parent_id, type, title, fields, created_at, updated_at, container_id \
         FROM blocks WHERE container_id IN (",
    );
    sql.bind_list(course_ids.iter().copied().map(Id))
        .push(") AND type IN (")
        .bind_list([BlockType::Unit.as_str(), BlockType::Chapter.as_str()])
        .push(")");
    let rows = scope.fetch_all(sql).await?;
    let mut by_course: HashMap<String, Vec<UnitNode>> = HashMap::new();
    for row in &rows {
        let course_id = row.id(7)?;
        let node = NodeRow::from_row(row)?.node;
        by_course.entry(course_id).or_default().push(node);
    }
    Ok(by_course)
}

async fn fetch_my_progress_for_courses(
    scope: &mut Scope<'_>,
    course_ids: &[&str],
    user_id: &str,
    org: Param,
) -> Result<BTreeMap<String, UnitProgress>, StoreError> {
    let mut sql = scope.sql(
        "SELECT up.unit_id, up.completed_at, up.planned_date, up.last_position, up.updated_at \
         FROM unit_progress AS up \
         WHERE up.user_id = ",
    );
    sql.bind(Id(user_id))
        .push(" AND up.container_id IN (")
        .bind_list(course_ids.iter().copied().map(Id))
        .push(")")
        .push_bind(" AND up.org_id = ", org);
    let rows = scope.fetch_all(sql).await?;
    let mut map = BTreeMap::new();
    for row in &rows {
        let progress = UnitProgress::from_row(row)?;
        map.insert(progress.unit_id.clone(), progress);
    }
    Ok(map)
}

impl FromRow for NodeRow {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let fields = row.json_map(4, "fields")?;
        let node = UnitNode::from(Block {
            id: row.id(0)?,
            parent_id: row.id(1)?,
            block_type: crate::blocks::block_type_column(row, 2, "type")?,
            title: row.text(3)?,
            fields: fields.clone(),
            created_at: row.int(5)?,
            updated_at: row.int(6)?,
            ..Block::default()
        });
        Ok(NodeRow { node, fields })
    }
}

impl FromRow for CourseSchedule {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let raw_days = row.int(2)?;
        let study_days = u8::try_from(raw_days)
            .ok()
            .and_then(StudyDays::try_new)
            .ok_or_else(|| StoreError::Corrupt {
                column: "study_days",
                reason: format!("invalid study_days bitmask: {raw_days}"),
            })?
            .bits();
        Ok(CourseSchedule {
            start_date: row.text(0)?,
            target_date: row.text(1)?,
            study_days,
            updated_at: row.int(3)?,
        })
    }
}

/// Forgets shared schedules of courses that are gone for good: neither live in
/// `containers` nor awaiting restore in `deleted_containers` of the row's organization.
pub(crate) async fn forget_gone_containers(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "DELETE FROM course_schedules WHERE NOT EXISTS \
         (SELECT 1 FROM containers AS c WHERE c.id = course_schedules.container_id",
    );
    sql.changing_no_access()
        .push(&scope.same_organization(&[("c", "course_schedules")]))
        .push(
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_containers AS d WHERE d.id = course_schedules.container_id",
        )
        .push(&scope.same_organization(&[("d", "course_schedules")]))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}
