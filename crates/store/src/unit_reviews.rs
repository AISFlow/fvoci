//! Personal spaced-repetition reviews (`unit_reviews`) and personal study notes
//! (`unit_notes`) on course units.
//!
//! Every personal read and write is scoped to the calling user (`user_id`) and the
//! handle's organization. Rows survive soft deletion of a unit or course into the trash
//! so restoration preserves them, and are cleaned up on permanent purge
//! ([`forget_gone_blocks`]), workspace removal ([`forget_user_in_workspace`]),
//! organization departure ([`forget_user_in_organization`]), and account erasure.

mod cleanup;
mod notes;

use std::collections::HashMap;

use fvoci_model::{
    BlockType, CompletedUnitReview, ContainerKind, NewUnitReview, NotificationKind,
    ReviewStatusFilter, TodayReviewItem, UnitReview, WorkspaceRole, days_between, is_id, new_id,
    suggested_next_interval_days,
};

pub(crate) use self::cleanup::{
    forget_gone_blocks, forget_user_in_organization, forget_user_in_workspace,
    forget_user_in_workspace_unjoined,
};
use crate::Store;
use crate::containers::inherited_members_source;
use crate::db::{Id, Param, Row, Scope};
use crate::error::StoreError;
use crate::notification_outbox::keep_newest;
use crate::unit_progress::require_live_unit;

/// A pending, un-notified review row inspected by background due-review housekeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueReviewCandidate {
    /// Review identifier.
    pub id: String,
    /// Organization identifier owning the review.
    pub org_id: String,
    /// Workspace identifier of the course.
    pub workspace_id: String,
    /// Course container identifier.
    pub container_id: String,
    /// Unit block identifier.
    pub unit_id: String,
    /// Learner who owns the review.
    pub user_id: String,
    /// Scheduled calendar date (`YYYY-MM-DD`).
    pub scheduled_date: String,
    /// Creation time in milliseconds since the Unix epoch (used for keyset paging).
    pub created_at: i64,
}

impl Store {
    /// Lists the calling learner's reviews for live units of `course_id`, ordered by
    /// `scheduled_date ASC, created_at ASC, id ASC`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn list_my_course_reviews(
        &self,
        course_id: &str,
        user_id: &str,
        status: ReviewStatusFilter,
        unit_id: Option<&str>,
    ) -> Result<Vec<UnitReview>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql(
                "SELECT ur.id, ur.unit_id, ur.container_id, ur.scheduled_date, \
                 ur.completed_at, CAST(ur.interval_days AS BIGINT), ur.created_at, ur.updated_at \
                 FROM unit_reviews AS ur \
                 JOIN blocks AS b ON b.id = ur.unit_id \
                 JOIN containers AS c ON c.id = b.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 WHERE ur.container_id = ",
            );
            sql.bind(Id(course_id))
                .push_bind(" AND ur.user_id = ", Id(user_id))
                .push_bind(" AND ur.org_id = ", org.clone())
                .push(" AND b.container_id = ur.container_id")
                .push_bind(" AND b.type = ", BlockType::Unit.as_str())
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND w.org_id = ", org)
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("ur", "b"), ("b", "c"), ("c", "w")]));
            match status {
                ReviewStatusFilter::All => {}
                ReviewStatusFilter::Pending => {
                    sql.push(" AND ur.completed_at IS NULL");
                }
                ReviewStatusFilter::Completed => {
                    sql.push(" AND ur.completed_at IS NOT NULL");
                }
            }
            if let Some(unit_id) = unit_id {
                sql.push_bind(" AND ur.unit_id = ", Id(unit_id));
            }
            sql.push(" ORDER BY ur.scheduled_date ASC, ur.created_at ASC, ur.id ASC");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(review_from_row)
                .collect()
        })
        .await
    }

    /// Returns the latest `interval_days` per live unit of `course_id` for `user_id`
    /// (optionally narrowed to `unit_id`), used to compute `suggestedNextIntervalDays`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn latest_unit_review_intervals(
        &self,
        course_id: &str,
        user_id: &str,
        unit_id: Option<&str>,
    ) -> Result<HashMap<String, u32>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql(
                "SELECT ur.unit_id, CAST(ur.interval_days AS BIGINT) \
                 FROM unit_reviews AS ur \
                 JOIN blocks AS b ON b.id = ur.unit_id \
                 JOIN containers AS c ON c.id = b.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 WHERE ur.container_id = ",
            );
            sql.bind(Id(course_id))
                .push_bind(" AND ur.user_id = ", Id(user_id))
                .push_bind(" AND ur.org_id = ", org.clone())
                .push(" AND b.container_id = ur.container_id")
                .push_bind(" AND b.type = ", BlockType::Unit.as_str())
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND w.org_id = ", org)
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("ur", "b"), ("b", "c"), ("c", "w")]));
            if let Some(unit_id) = unit_id {
                sql.push_bind(" AND ur.unit_id = ", Id(unit_id));
            }
            sql.push(" ORDER BY COALESCE(ur.completed_at, ur.created_at) DESC, ur.created_at DESC, ur.id DESC");
            let mut latest = HashMap::new();
            for row in scope.fetch_all(sql).await? {
                let unit = row.id(0)?;
                let interval = positive_u32(row.int(1)?);
                latest.entry(unit).or_insert(interval);
            }
            Ok(latest)
        })
        .await
    }

    /// Schedules a new personal review on a live unit of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `unit_id` is not a live unit of `course_id` or the
    /// user is not an active workspace member, and [`StoreError::Database`] if a
    /// statement fails.
    pub async fn schedule_unit_review(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        new: &NewUnitReview,
        now: Option<i64>,
    ) -> Result<UnitReview, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_unit(scope, course_id, unit_id, user_id, org.clone()).await?;
            let now = now.unwrap_or_else(|| scope.now_millis());
            insert_review(scope, course_id, unit_id, user_id, org, new, now).await
        })
        .await
    }

    /// Marks the caller's review `review_id` as completed (idempotently preserving any
    /// existing `completed_at` and avoiding duplicate follow-up reviews on replay),
    /// computes `suggested_next_interval_days`, and optionally schedules a follow-up
    /// review atomically in the same transaction when transitioning from pending.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the review does not exist for this caller and course
    /// or its unit is no longer live, and [`StoreError::Database`] if a statement fails.
    pub async fn complete_unit_review(
        &self,
        course_id: &str,
        review_id: &str,
        user_id: &str,
        intervals: &[u32],
        next: Option<&NewUnitReview>,
        now: Option<i64>,
    ) -> Result<CompletedUnitReview, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        if !is_id(review_id) {
            return Err(StoreError::not_found("unit review"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            let unit_id =
                find_live_review_unit(scope, course_id, review_id, user_id, org.clone()).await?;
            // Acquire `blocks -> workspace_members -> users` before locking `unit_reviews`
            // so concurrent removal/erasure cannot deadlock on PostgreSQL.
            require_live_unit(scope, course_id, &unit_id, user_id, org.clone()).await?;
            let now = now.unwrap_or_else(|| scope.now_millis());
            let mut update = scope.sql("UPDATE unit_reviews SET completed_at = ");
            update
                .changing_no_access()
                .bind(now)
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(review_id))
                .push_bind(" AND container_id = ", Id(course_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND org_id = ", org.clone())
                .push(" AND completed_at IS NULL")
                .push(
                    " RETURNING id, unit_id, container_id, scheduled_date, completed_at, \
                     CAST(interval_days AS BIGINT), created_at, updated_at",
                );
            let (review, transitioned) = match scope.fetch_optional(update).await? {
                Some(row) => (review_from_row(&row)?, true),
                None => (
                    fetch_review_row(scope, course_id, review_id, user_id, org.clone()).await?,
                    false,
                ),
            };
            let suggested = suggested_next_interval_days(Some(review.interval_days), intervals);
            let next_review = match (transitioned, next) {
                (true, Some(next_new)) => Some(
                    insert_review(scope, course_id, &unit_id, user_id, org, next_new, now).await?,
                ),
                _ => None,
            };
            Ok(CompletedUnitReview {
                review,
                suggested_next_interval_days: suggested,
                next_review,
            })
        })
        .await
    }

    /// Deletes the caller's review `review_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if no such review belongs to `user_id` in `course_id`,
    /// and [`StoreError::Database`] if the statement fails.
    pub async fn delete_unit_review(
        &self,
        course_id: &str,
        review_id: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        if !is_id(review_id) {
            return Err(StoreError::not_found("unit review"));
        }
        self.with_write_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql("DELETE FROM unit_reviews WHERE id = ");
            sql.changing_no_access()
                .bind(Id(review_id))
                .push_bind(" AND container_id = ", Id(course_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND org_id = ", org);
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("unit review"));
            }
            Ok(())
        })
        .await
    }

    /// Lists the calling learner's pending reviews due on or before `today` across live
    /// courses of `workspace_id` in which the caller still holds active membership,
    /// ordered by `scheduled_date ASC, created_at ASC, id ASC`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn list_workspace_today_reviews(
        &self,
        workspace_id: &str,
        user_id: &str,
        today: &str,
    ) -> Result<Vec<TodayReviewItem>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let inherited = inherited_members_source(scope);
            let mut sql = scope.sql(
                "SELECT ur.id, ur.unit_id, ur.container_id, c.title, b.title, \
                 ur.scheduled_date, CAST(ur.interval_days AS BIGINT), ur.created_at, \
                 ur.updated_at, c.properties \
                 FROM unit_reviews AS ur \
                 JOIN blocks AS b ON b.id = ur.unit_id \
                 JOIN containers AS c ON c.id = b.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 JOIN workspace_members AS wm \
                   ON wm.workspace_id = w.id AND wm.user_id = ur.user_id \
                 WHERE ur.user_id = ",
            );
            sql.bind(Id(user_id))
                .push_bind(" AND ur.org_id = ", org.clone())
                .push(" AND ur.completed_at IS NULL")
                .push_bind(" AND ur.scheduled_date <= ", today)
                .push(" AND b.container_id = ur.container_id")
                .push_bind(" AND b.type = ", BlockType::Unit.as_str())
                .push_bind(" AND c.workspace_id = ", Id(workspace_id))
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push_bind(" AND w.org_id = ", org.clone())
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("ur", "b"), ("b", "c"), ("c", "w"), ("wm", "w")]))
                .push(
                    " AND (EXISTS (\
                     SELECT 1 FROM container_members AS cm \
                     WHERE cm.container_id = c.id AND cm.user_id = ur.user_id",
                )
                .push(&scope.same_organization(&[("cm", "c")]))
                .push(") OR (wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push(" AND EXISTS (SELECT 1 FROM ")
                .push(&inherited)
                .push(" AS im WHERE im.container_id = c.id AND im.user_id = ur.user_id")
                .push_bind(" AND im.org_id = ", org)
                .push(&scope.same_organization(&[("im", "c")]))
                .push("))) ORDER BY ur.scheduled_date ASC, ur.created_at ASC, ur.id ASC");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| today_review_from_row(row, today))
                .collect()
        })
        .await
    }

    /// Returns up to `limit` pending, un-notified reviews whose `scheduled_date` is on or
    /// before `max_scheduled_date` for learners who still hold active workspace and course
    /// membership across every organization reachable by this handle (intended for a
    /// system-scoped housekeeping handle).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn due_unnotified_reviews(
        &self,
        max_scheduled_date: &str,
        after: Option<(&str, i64, &str)>,
        limit: i64,
    ) -> Result<Vec<DueReviewCandidate>, StoreError> {
        self.with_conn(async |scope| {
            let inherited = inherited_members_source(scope);
            let mut sql = scope.sql(
                "SELECT ur.id, w.org_id, c.workspace_id, ur.container_id, ur.unit_id, \
                 ur.user_id, ur.scheduled_date, ur.created_at \
                 FROM unit_reviews AS ur \
                 JOIN blocks AS b ON b.id = ur.unit_id \
                 JOIN containers AS c ON c.id = b.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 JOIN users AS u ON u.id = ur.user_id \
                 JOIN workspace_members AS wm \
                   ON wm.workspace_id = w.id AND wm.user_id = ur.user_id \
                 WHERE ur.completed_at IS NULL \
                   AND ur.notified_at IS NULL \
                   AND ur.scheduled_date <= ",
            );
            sql.bind(max_scheduled_date)
                .push(" AND ur.org_id = w.org_id AND b.container_id = ur.container_id")
                .push_bind(" AND b.type = ", BlockType::Unit.as_str())
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(" AND u.deleted_at IS NULL")
                .push_bind(" AND u.erased_at = ", 0_i64)
                .push(&scope.same_organization(&[("ur", "b"), ("b", "c"), ("c", "w"), ("wm", "w")]))
                .push(
                    " AND (EXISTS (\
                     SELECT 1 FROM container_members AS cm \
                     WHERE cm.container_id = c.id AND cm.user_id = ur.user_id",
                )
                .push(&scope.same_organization(&[("cm", "c")]))
                .push(") OR (wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push(" AND EXISTS (SELECT 1 FROM ")
                .push(&inherited)
                .push(
                    " AS im WHERE im.container_id = c.id AND im.user_id = ur.user_id \
                     AND im.org_id = w.org_id",
                )
                .push(&scope.same_organization(&[("im", "c")]))
                .push(")))");
            if let Some((date, created_at, id)) = after {
                sql.push_bind(" AND (ur.scheduled_date > ", date)
                    .push_bind(" OR (ur.scheduled_date = ", date)
                    .push_bind(" AND (ur.created_at > ", created_at)
                    .push_bind(" OR (ur.created_at = ", created_at)
                    .push_bind(" AND ur.id > ", Id(id))
                    .push("))))");
            }
            sql.push_bind(
                " ORDER BY ur.scheduled_date ASC, ur.created_at ASC, ur.id ASC LIMIT ",
                limit,
            );
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(DueReviewCandidate {
                        id: row.id(0)?,
                        org_id: row.id(1)?,
                        workspace_id: row.id(2)?,
                        container_id: row.id(3)?,
                        unit_id: row.id(4)?,
                        user_id: row.id(5)?,
                        scheduled_date: row.text(6)?,
                        created_at: row.int(7)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Atomically marks `candidate` notified at `now` and inserts a single `review_due`
    /// notification addressed strictly to `candidate.user_id`. Returns `false` if the
    /// review was already notified or completed concurrently.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is committed then.
    pub async fn notify_due_review(
        &self,
        candidate: &DueReviewCandidate,
        now: i64,
        most_of_a_person: i64,
    ) -> Result<bool, StoreError> {
        self.transaction(async |scope| {
            let mut mark = scope.sql("UPDATE unit_reviews SET notified_at = ");
            mark.changing_no_access()
                .bind(now)
                .push_bind(" WHERE id = ", Id(&candidate.id))
                .push_bind(" AND user_id = ", Id(&candidate.user_id))
                .push_bind(" AND org_id = ", Id(&candidate.org_id))
                .push(" AND completed_at IS NULL AND notified_at IS NULL");
            if scope.execute(mark).await? == 0 {
                return Ok(false);
            }
            let notif_id = new_id();
            let mut insert = scope.sql(
                "INSERT INTO notifications (recipient_id, id, org_id, kind, source_id, \
                 block_id, container_id, actor_id, made_by, created_at, updated_at, times, \
                 doers, counted, read_at) VALUES (",
            );
            insert
                .bind(Id(&candidate.user_id))
                .push_bind(", ", Id(&notif_id))
                .push_bind(", ", Id(&candidate.org_id))
                .push_bind(", ", NotificationKind::ReviewDue.as_str())
                .push_bind(", ", Id(&candidate.unit_id))
                .push_bind(", ", Id(&candidate.unit_id))
                .push_bind(", ", Id(&candidate.container_id))
                .push(", NULL, NULL")
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push(", 1, '[]', '[]', 0)");
            scope.execute(insert).await?;
            keep_newest(scope, &candidate.user_id, most_of_a_person).await?;
            Ok(true)
        })
        .await
    }
}

async fn insert_review(
    scope: &mut Scope<'_>,
    course_id: &str,
    unit_id: &str,
    user_id: &str,
    org: Param,
    new: &NewUnitReview,
    now: i64,
) -> Result<UnitReview, StoreError> {
    let id = new_id();
    let mut sql = scope.sql(
        "INSERT INTO unit_reviews \
         (id, org_id, unit_id, container_id, user_id, scheduled_date, completed_at, \
          interval_days, notified_at, created_at, updated_at) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(&id))
        .push_bind(", ", org)
        .push_bind(", ", Id(unit_id))
        .push_bind(", ", Id(course_id))
        .push_bind(", ", Id(user_id))
        .push_bind(", ", new.scheduled_date.as_str())
        .push(", NULL")
        .push_bind(", ", i64::from(new.interval_days))
        .push(", NULL")
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(
            ") RETURNING id, unit_id, container_id, scheduled_date, completed_at, \
             CAST(interval_days AS BIGINT), created_at, updated_at",
        );
    let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
    review_from_row(&row)
}

async fn find_live_review_unit(
    scope: &mut Scope<'_>,
    course_id: &str,
    review_id: &str,
    user_id: &str,
    org: Param,
) -> Result<String, StoreError> {
    let mut sql = scope.sql(
        "SELECT ur.unit_id \
         FROM unit_reviews AS ur \
         JOIN blocks AS b ON b.id = ur.unit_id \
         JOIN containers AS c ON c.id = b.container_id \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE ur.id = ",
    );
    sql.bind(Id(review_id))
        .push_bind(" AND ur.container_id = ", Id(course_id))
        .push_bind(" AND ur.user_id = ", Id(user_id))
        .push_bind(" AND ur.org_id = ", org.clone())
        .push(" AND b.container_id = ur.container_id")
        .push_bind(" AND b.type = ", BlockType::Unit.as_str())
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("ur", "b"), ("b", "c"), ("c", "w")]));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("unit review"))?;
    row.id(0)
}

async fn fetch_review_row(
    scope: &mut Scope<'_>,
    course_id: &str,
    review_id: &str,
    user_id: &str,
    org: Param,
) -> Result<UnitReview, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, unit_id, container_id, scheduled_date, completed_at, \
         CAST(interval_days AS BIGINT), created_at, updated_at \
         FROM unit_reviews \
         WHERE id = ",
    );
    sql.bind(Id(review_id))
        .push_bind(" AND container_id = ", Id(course_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push_bind(" AND org_id = ", org);
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("unit review"))?;
    review_from_row(&row)
}

fn review_from_row(row: &Row) -> Result<UnitReview, StoreError> {
    Ok(UnitReview {
        id: row.id(0)?,
        unit_id: row.id(1)?,
        container_id: row.id(2)?,
        scheduled_date: row.text(3)?,
        completed_at: row.opt_int(4)?,
        interval_days: positive_u32(row.int(5)?),
        created_at: row.int(6)?,
        updated_at: row.int(7)?,
    })
}

fn today_review_from_row(row: &Row, today: &str) -> Result<TodayReviewItem, StoreError> {
    let scheduled_date = row.text(5)?;
    let overdue = days_between(&scheduled_date, today).unwrap_or(0).max(0);
    let interval_days = positive_u32(row.int(6)?);
    let properties = row.json_map(9, "properties")?;
    let review_intervals = fvoci_model::course_review_intervals(&properties);
    let suggested = suggested_next_interval_days(Some(interval_days), &review_intervals);
    Ok(TodayReviewItem {
        id: row.id(0)?,
        unit_id: row.id(1)?,
        container_id: row.id(2)?,
        course_title: row.text(3)?,
        unit_title: row.text(4)?,
        scheduled_date,
        interval_days,
        overdue_days: u32::try_from(overdue).unwrap_or(u32::MAX),
        created_at: row.int(7)?,
        updated_at: row.int(8)?,
        suggested_next_interval_days: suggested,
        review_intervals,
    })
}

fn positive_u32(value: i64) -> u32 {
    u32::try_from(value.max(1)).unwrap_or(u32::MAX)
}
