//! Course exams (`course_exams`) and strictly personal exam scores (`exam_scores`).
//!
//! Individual scores and notes are never returned for any user other than the authenticated
//! caller. Aggregate statistics (`average_score`, `published_score_bucket`) are disclosed
//! only from a frozen snapshot created when a course administrator explicitly publishes
//! results (`publish_course_exam_results`), requiring at least [`MIN_SCORERS_TO_PUBLISH`]
//! active scorers and rounded to 0.1 / buckets of 5 so neither learners nor administrators
//! can reverse-engineer an individual score via time-differencing.
//!
//! Rows survive soft deletion of a course into the trash and are removed on permanent
//! purge ([`forget_gone_containers`]). Personal `exam_scores` rows are deleted when a user
//! is removed from a workspace ([`forget_user_in_workspace`]), ends an org-admin joining
//! ([`forget_user_in_workspace_unjoined`]), leaves an organization
//! ([`forget_user_in_organization`]), or has their account erased.

use std::collections::BTreeMap;

use fvoci_model::{
    ContainerKind, CourseExam, CourseExamChange, ExamDateChange, MyExamScore, ValidatedMyExamScore,
    ValidatedNewExam, is_id, new_id,
};

use crate::Store;
use crate::course_cleanup::{self, PersonalCourseTable};
use crate::course_meetings::{
    push_active_course_members_subquery, require_active_course_member, require_live_course,
};
use crate::db::{Dialect, Id, Param, Row, Scope};
use crate::error::StoreError;

/// Minimum active enrolled course members with a recorded score required to publish an
/// exam result snapshot.
pub const MIN_SCORERS_TO_PUBLISH: u32 = 5;

/// Minimum growth in active scorer count over the last published snapshot required
/// before recomputing exam results. A count that fell, or rose by less, never yields a
/// new snapshot: two snapshots one scorer apart would give away that scorer's score.
pub const MIN_SCORER_DELTA_TO_REPUBLISH: u32 = 3;

/// Caller perspective determining which exam aggregates and personal scores are returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExamViewer<'a> {
    /// Unjoined open-course reader (`score_count = 0`, no personal score, no snapshot).
    Unjoined,
    /// Enrolled non-manager course member (`score_count` floored to a multiple of `5` so
    /// exact scorer increments remain hidden, caller's own `my_score`, and frozen snapshot
    /// when published).
    Learner(&'a str),
    /// Enrolled course role manager (`ManageRoles`: exact `score_count` so publication
    /// readiness is visible, caller's own `my_score`, and frozen snapshot when published).
    Manager(&'a str),
}

impl<'a> ExamViewer<'a> {
    const fn user_id(self) -> Option<&'a str> {
        match self {
            Self::Unjoined => None,
            Self::Learner(uid) | Self::Manager(uid) => Some(uid),
        }
    }
}

/// Outcome of recording the caller's personal exam score.
#[derive(Debug, Clone, PartialEq)]
pub enum ExamScoreOutcome {
    /// The caller's score was saved.
    Saved(MyExamScore),
    /// The score exceeded the exam's configured `max_score`.
    ExceedsMaxScore {
        /// The exam's current maximum score.
        max_score: f64,
    },
}

/// Reason publishing or republishing exam results was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExamPublishError {
    /// Fewer than [`MIN_SCORERS_TO_PUBLISH`] active course members have recorded a score.
    TooFewScorers {
        /// Current active scorer count.
        active_scorers: u32,
        /// Minimum active scorers required (`5`).
        minimum: u32,
    },
    /// Results are published and the active scorer count has not grown by at least
    /// [`MIN_SCORER_DELTA_TO_REPUBLISH`] over the previous snapshot (a count that fell
    /// counts as no growth).
    TooFewChangesSincePublish {
        /// Current active scorer count.
        current_scorers: u32,
        /// Active scorer count recorded at the previous publication.
        previous_scorers: u32,
        /// Minimum growth over the previous snapshot required (`3`).
        minimum_delta: u32,
    },
}

impl std::fmt::Display for ExamPublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewScorers {
                active_scorers,
                minimum,
            } => write!(
                f,
                "exam results require at least {minimum} active member scores to publish (currently {active_scorers})"
            ),
            Self::TooFewChangesSincePublish {
                current_scorers,
                previous_scorers,
                minimum_delta,
            } => write!(
                f,
                "republishing exam results requires at least {minimum_delta} more active member scores than the last snapshot ({previous_scorers} -> {current_scorers})"
            ),
        }
    }
}

impl std::error::Error for ExamPublishError {}

struct StoredExam {
    exam: CourseExam,
    results_published_at: Option<i64>,
    published_average_score: Option<f64>,
    published_score_bucket: Option<u32>,
    published_submission_count: Option<u32>,
}

impl StoredExam {
    fn into_unjoined_view(self) -> CourseExam {
        self.exam
    }

    fn into_view(
        mut self,
        viewer: ExamViewer<'_>,
        live_score_count: u32,
        my_score: Option<MyExamScore>,
    ) -> CourseExam {
        match viewer {
            ExamViewer::Unjoined => return self.exam,
            ExamViewer::Learner(_) => {
                self.exam.score_count = (live_score_count / 5) * 5;
            }
            ExamViewer::Manager(_) => {
                self.exam.score_count = live_score_count;
            }
        }
        self.exam.my_score = my_score;
        if self.results_published_at.is_some() {
            self.exam.results_published_at = self.results_published_at;
            self.exam.average_score = self.published_average_score;
            self.exam.published_score_bucket = self.published_score_bucket;
        }
        self.exam
    }
}

impl Store {
    /// Lists all exams of live course `course_id`, ordered by `exam_date ASC NULLS LAST,
    /// created_at ASC, id ASC`.
    ///
    /// Returns `my_score` strictly for the caller when enrolled, `score_count` floored to a
    /// multiple of `5` for [`ExamViewer::Learner`] or exact for [`ExamViewer::Manager`],
    /// and the frozen snapshot (`average_score`, `results_published_at`,
    /// `published_score_bucket`) if and only if results are currently published.
    /// [`ExamViewer::Unjoined`] omits `my_score` and suppresses all aggregates.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if a query fails.
    pub async fn list_course_exams(
        &self,
        course_id: &str,
        viewer: ExamViewer<'_>,
    ) -> Result<Vec<CourseExam>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql(
                "SELECT e.id, e.container_id, e.title, e.exam_date, e.max_score, \
                 e.description, e.created_at, e.updated_at, e.results_published_at, \
                 e.published_average_score, e.published_score_bucket, \
                 e.published_submission_count \
                 FROM course_exams AS e \
                 JOIN containers AS c ON c.id = e.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 WHERE e.container_id = ",
            );
            sql.bind(Id(course_id))
                .push_bind(" AND e.org_id = ", org.clone())
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND w.org_id = ", org.clone())
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("e", "c"), ("c", "w")]))
                .push(
                    " ORDER BY CASE WHEN e.exam_date IS NULL THEN 1 ELSE 0 END, \
                     e.exam_date ASC, e.created_at ASC, e.id ASC",
                );
            let rows = scope.fetch_all(sql).await?;
            let mut stored = Vec::with_capacity(rows.len());
            for row in &rows {
                stored.push(stored_exam_from_row(row)?);
            }
            if stored.is_empty() {
                return Ok(Vec::new());
            }
            let Some(uid) = viewer.user_id() else {
                return Ok(stored
                    .into_iter()
                    .map(StoredExam::into_unjoined_view)
                    .collect());
            };
            let mut counts = fetch_exam_counts(scope, course_id, &org, None).await?;
            let mut my_scores = fetch_my_scores(scope, course_id, &org, uid, None).await?;
            Ok(stored
                .into_iter()
                .map(|item| {
                    let count = counts.remove(&item.exam.id).unwrap_or(0);
                    let mine = my_scores.remove(&item.exam.id);
                    item.into_view(viewer, count, mine)
                })
                .collect())
        })
        .await
    }

    /// Creates a new exam on live course `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn create_course_exam(
        &self,
        course_id: &str,
        new: &ValidatedNewExam,
    ) -> Result<CourseExam, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let id = new_id();
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO course_exams \
                 (id, org_id, container_id, title, exam_date, max_score, description, \
                  created_at, updated_at) VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(&id))
                .push_bind(", ", org)
                .push_bind(", ", Id(course_id))
                .push_bind(", ", &new.title)
                .push_bind(", ", Param::OptText(new.exam_date.clone()))
                .push_bind(", ", new.max_score)
                .push_bind(", ", &new.description)
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push(")");
            scope.execute(sql).await?;
            Ok(CourseExam {
                id,
                container_id: course_id.to_owned(),
                title: new.title.clone(),
                exam_date: new.exam_date.clone(),
                max_score: new.max_score,
                description: new.description.clone(),
                created_at: now,
                updated_at: now,
                my_score: None,
                average_score: None,
                score_count: 0,
                results_published_at: None,
                published_score_bucket: None,
            })
        })
        .await
    }

    /// Updates an exam of live course `course_id` and returns the updated view for
    /// `viewer` under the same snapshot visibility rules as [`Store::list_course_exams`].
    ///
    /// Changing `max_score` intentionally does not inspect other members' stored scores
    /// so it cannot be used as an oracle to probe private scores.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn patch_course_exam(
        &self,
        course_id: &str,
        exam_id: &str,
        viewer: ExamViewer<'_>,
        change: &CourseExamChange,
    ) -> Result<CourseExam, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut existing = fetch_exam_for_update(scope, course_id, exam_id, &org).await?;
            let now = scope.now_millis();
            let title = change
                .title
                .as_deref()
                .unwrap_or(&existing.exam.title)
                .to_owned();
            let exam_date = match &change.exam_date {
                ExamDateChange::Keep => existing.exam.exam_date.clone(),
                ExamDateChange::Clear => None,
                ExamDateChange::Set(date) => Some(date.clone()),
            };
            let max_score = change.max_score.unwrap_or(existing.exam.max_score);
            let description = change
                .description
                .as_deref()
                .unwrap_or(&existing.exam.description)
                .to_owned();
            let mut sql = scope.sql("UPDATE course_exams SET title = ");
            sql.changing_no_access()
                .bind(&title)
                .push_bind(", exam_date = ", Param::OptText(exam_date.clone()))
                .push_bind(", max_score = ", max_score)
                .push_bind(", description = ", &description)
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(exam_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            scope.execute(sql).await?;

            existing.exam.title = title;
            existing.exam.exam_date = exam_date;
            existing.exam.max_score = max_score;
            existing.exam.description = description;
            existing.exam.updated_at = now;

            view_for_single_exam(scope, course_id, &org, exam_id, viewer, existing, None).await
        })
        .await
    }

    /// Publishes or republishes the coarse exam result snapshot (`averageScore` rounded to
    /// `0.1`, `publishedScoreBucket` rounded down to a multiple of `5`) on live course
    /// `course_id`.
    ///
    /// Requires at least [`MIN_SCORERS_TO_PUBLISH`] active enrolled course members with a
    /// recorded score. When a previous snapshot exists, recomputing requires the scorer
    /// count to have grown by at least [`MIN_SCORER_DELTA_TO_REPUBLISH`] over it, so the
    /// snapshots of one exam only ever count more scorers (5 → 9 → 6 would leave the first
    /// and the last one scorer apart). Otherwise, while currently published, the call is
    /// refused with [`ExamPublishError::TooFewChangesSincePublish`]; while currently
    /// unpublished, it restores the frozen snapshot without recomputing, so
    /// unpublish/republish cannot probe a single score either.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn publish_course_exam_results(
        &self,
        course_id: &str,
        exam_id: &str,
        viewer: ExamViewer<'_>,
    ) -> Result<Result<CourseExam, ExamPublishError>, StoreError> {
        if !is_id(exam_id) {
            return Err(StoreError::not_found("exam"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut existing = fetch_exam_for_update(scope, course_id, exam_id, &org).await?;
            let (count, raw_avg) =
                fetch_single_exam_aggregate(scope, course_id, &org, exam_id).await?;
            if count < MIN_SCORERS_TO_PUBLISH {
                return Ok(Err(ExamPublishError::TooFewScorers {
                    active_scorers: count,
                    minimum: MIN_SCORERS_TO_PUBLISH,
                }));
            }
            if let Some(previous_scorers) = existing.published_submission_count
                && count < previous_scorers.saturating_add(MIN_SCORER_DELTA_TO_REPUBLISH)
            {
                if existing.results_published_at.is_some() {
                    return Ok(Err(ExamPublishError::TooFewChangesSincePublish {
                        current_scorers: count,
                        previous_scorers,
                        minimum_delta: MIN_SCORER_DELTA_TO_REPUBLISH,
                    }));
                }
                let now = scope.now_millis();
                republish_frozen(scope, course_id, exam_id, &org, &mut existing, now).await?;
                let view = view_for_single_exam(
                    scope,
                    course_id,
                    &org,
                    exam_id,
                    viewer,
                    existing,
                    Some(count),
                )
                .await?;
                return Ok(Ok(view));
            }
            let rounded = (raw_avg.unwrap_or(0.0) * 10.0).round() / 10.0;
            let rounded_avg = if rounded == 0.0 { 0.0 } else { rounded };
            let bucket = (count / 5) * 5;
            let now = scope.now_millis();

            let mut sql = scope.sql("UPDATE course_exams SET results_published_at = ");
            sql.changing_no_access()
                .bind(now)
                .push_bind(", published_average_score = ", rounded_avg)
                .push_bind(", published_score_bucket = ", i64::from(bucket))
                .push_bind(", published_submission_count = ", i64::from(count))
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(exam_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            scope.execute(sql).await?;

            existing.exam.updated_at = now;
            existing.results_published_at = Some(now);
            existing.published_average_score = Some(rounded_avg);
            existing.published_score_bucket = Some(bucket);
            existing.published_submission_count = Some(count);

            let view = view_for_single_exam(
                scope,
                course_id,
                &org,
                exam_id,
                viewer,
                existing,
                Some(count),
            )
            .await?;
            Ok(Ok(view))
        })
        .await
    }

    /// Unpublishes the exam result snapshot on live course `course_id`, hiding the snapshot
    /// (`results_published_at = NULL`) while preserving the last snapshot's baseline so
    /// subsequent republication still enforces [`MIN_SCORER_DELTA_TO_REPUBLISH`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn unpublish_course_exam_results(
        &self,
        course_id: &str,
        exam_id: &str,
        viewer: ExamViewer<'_>,
    ) -> Result<CourseExam, StoreError> {
        if !is_id(exam_id) {
            return Err(StoreError::not_found("exam"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut existing = fetch_exam_for_update(scope, course_id, exam_id, &org).await?;
            let now = scope.now_millis();

            let mut sql =
                scope.sql("UPDATE course_exams SET results_published_at = NULL, updated_at = ");
            sql.changing_no_access()
                .bind(now)
                .push_bind(" WHERE id = ", Id(exam_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            scope.execute(sql).await?;

            existing.exam.updated_at = now;
            existing.results_published_at = None;

            view_for_single_exam(scope, course_id, &org, exam_id, viewer, existing, None).await
        })
        .await
    }

    /// Permanently deletes an exam of live course `course_id` and all associated
    /// `exam_scores` rows.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn delete_course_exam(
        &self,
        course_id: &str,
        exam_id: &str,
    ) -> Result<(), StoreError> {
        if !is_id(exam_id) {
            return Err(StoreError::not_found("exam"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut sql = scope.sql("DELETE FROM course_exams WHERE id = ");
            sql.changing_no_access()
                .bind(Id(exam_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("exam"));
            }
            let mut sql = scope.sql("DELETE FROM exam_scores WHERE exam_id = ");
            sql.changing_no_access()
                .bind(Id(exam_id))
                .push_bind(" AND org_id = ", org);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Creates or updates the calling learner's own personal score on `exam_id` of live
    /// course `course_id`.
    ///
    /// Enforces `0 <= score <= exam.max_score` and active course enrollment atomically
    /// inside the transaction while locking the exam, workspace membership, and user rows
    /// on PostgreSQL.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist or `user_id` is not
    /// an active enrolled member of `course_id`, and [`StoreError::Database`] if a
    /// statement fails.
    pub async fn upsert_my_exam_score(
        &self,
        course_id: &str,
        exam_id: &str,
        user_id: &str,
        input: &ValidatedMyExamScore,
    ) -> Result<ExamScoreOutcome, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            let (workspace_id, max_score) =
                require_live_exam(scope, course_id, exam_id, &org).await?;
            require_active_workspace_user(scope, &workspace_id, user_id).await?;
            require_active_course_member(scope, course_id, user_id, org.clone()).await?;
            if !input.score.is_finite() || input.score < 0.0 || input.score > max_score {
                return Ok(ExamScoreOutcome::ExceedsMaxScore { max_score });
            }
            let score = if input.score == 0.0 { 0.0 } else { input.score };
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO exam_scores \
                 (exam_id, org_id, container_id, user_id, score, note, updated_at) VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(exam_id))
                .push_bind(", ", org)
                .push_bind(", ", Id(course_id))
                .push_bind(", ", Id(user_id))
                .push_bind(", ", score)
                .push_bind(", ", &input.note)
                .push_bind(", ", now)
                .push(
                    ") ON CONFLICT (exam_id, org_id, user_id) \
                     DO UPDATE SET score = excluded.score, note = excluded.note, \
                     updated_at = excluded.updated_at",
                );
            scope.execute(sql).await?;
            Ok(ExamScoreOutcome::Saved(MyExamScore {
                score,
                note: input.note.clone(),
                updated_at: now,
            }))
        })
        .await
    }

    /// Deletes the calling learner's own personal score on `exam_id` of live course
    /// `course_id`. Succeeds idempotently if the exam exists and the caller has no score.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or exam does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn delete_my_exam_score(
        &self,
        course_id: &str,
        exam_id: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_exam(scope, course_id, exam_id, &org).await?;
            let mut sql = scope.sql("DELETE FROM exam_scores WHERE exam_id = ");
            sql.changing_no_access()
                .bind(Id(exam_id))
                .push_bind(" AND org_id = ", org)
                .push_bind(" AND user_id = ", Id(user_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }
}

async fn republish_frozen(
    scope: &mut Scope<'_>,
    course_id: &str,
    exam_id: &str,
    org: &Param,
    existing: &mut StoredExam,
    now: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE course_exams SET results_published_at = ");
    sql.changing_no_access()
        .bind(now)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE id = ", Id(exam_id))
        .push_bind(" AND org_id = ", org.clone())
        .push_bind(" AND container_id = ", Id(course_id));
    scope.execute(sql).await?;
    existing.exam.updated_at = now;
    existing.results_published_at = Some(now);
    Ok(())
}

async fn view_for_single_exam(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: &Param,
    exam_id: &str,
    viewer: ExamViewer<'_>,
    existing: StoredExam,
    known_count: Option<u32>,
) -> Result<CourseExam, StoreError> {
    let Some(uid) = viewer.user_id() else {
        return Ok(existing.into_unjoined_view());
    };
    let count = match known_count {
        Some(c) => c,
        None => fetch_exam_counts(scope, course_id, org, Some(exam_id))
            .await?
            .remove(exam_id)
            .unwrap_or(0),
    };
    let mine = fetch_my_scores(scope, course_id, org, uid, Some(exam_id))
        .await?
        .remove(exam_id);
    Ok(existing.into_view(viewer, count, mine))
}

async fn require_live_exam(
    scope: &mut Scope<'_>,
    course_id: &str,
    exam_id: &str,
    org: &Param,
) -> Result<(String, f64), StoreError> {
    let mut sql = scope.sql(
        "SELECT c.workspace_id, e.max_score \
         FROM course_exams AS e \
         JOIN containers AS c ON c.id = e.container_id \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE e.id = ",
    );
    sql.bind(Id(exam_id))
        .push_bind(" AND e.org_id = ", org.clone())
        .push_bind(" AND e.container_id = ", Id(course_id))
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org.clone())
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("e", "c"), ("c", "w")]));
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE OF e");
    }
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Err(StoreError::not_found("exam"));
    };
    Ok((row.id(0)?, row.float(1)?))
}

async fn require_active_workspace_user(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut member = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    member
        .bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    if scope.dialect() == Dialect::Postgres {
        member.push(" FOR SHARE");
    }
    if scope.fetch_optional(member).await?.is_none() {
        return Err(StoreError::not_found("exam"));
    }
    let mut user = scope.sql("SELECT 1 FROM users WHERE id = ");
    user.bind(Id(user_id))
        .push(" AND deleted_at IS NULL")
        .push_bind(" AND erased_at = ", 0_i64);
    if scope.dialect() == Dialect::Postgres {
        user.push(" FOR KEY SHARE");
    }
    if scope.fetch_optional(user).await?.is_none() {
        return Err(StoreError::not_found("exam"));
    }
    Ok(())
}

async fn fetch_exam_for_update(
    scope: &mut Scope<'_>,
    course_id: &str,
    exam_id: &str,
    org: &Param,
) -> Result<StoredExam, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, container_id, title, exam_date, max_score, description, created_at, \
         updated_at, results_published_at, published_average_score, published_score_bucket, \
         published_submission_count FROM course_exams WHERE id = ",
    );
    sql.bind(Id(exam_id))
        .push_bind(" AND org_id = ", org.clone())
        .push_bind(" AND container_id = ", Id(course_id));
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Err(StoreError::not_found("exam"));
    };
    stored_exam_from_row(&row)
}

async fn fetch_exam_counts(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: &Param,
    exam_id: Option<&str>,
) -> Result<BTreeMap<String, u32>, StoreError> {
    let mut sql = scope.sql(
        "SELECT es.exam_id, COUNT(*) \
         FROM exam_scores AS es \
         JOIN ",
    );
    push_active_course_members_subquery(scope, &mut sql, course_id, org.clone());
    sql.push(" AS am ON am.user_id = es.user_id WHERE es.container_id = ")
        .bind(Id(course_id))
        .push_bind(" AND es.org_id = ", org.clone());
    if let Some(eid) = exam_id {
        sql.push_bind(" AND es.exam_id = ", Id(eid));
    }
    sql.push(" GROUP BY es.exam_id");
    let mut counts = BTreeMap::new();
    for row in scope.fetch_all(sql).await? {
        let count = u32::try_from(row.int(1)?.max(0)).unwrap_or(u32::MAX);
        if count > 0 {
            counts.insert(row.id(0)?, count);
        }
    }
    Ok(counts)
}

async fn fetch_single_exam_aggregate(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: &Param,
    exam_id: &str,
) -> Result<(u32, Option<f64>), StoreError> {
    let mut sql = scope.sql(
        "SELECT COUNT(*), AVG(es.score) \
         FROM exam_scores AS es \
         JOIN ",
    );
    push_active_course_members_subquery(scope, &mut sql, course_id, org.clone());
    sql.push(" AS am ON am.user_id = es.user_id WHERE es.container_id = ")
        .bind(Id(course_id))
        .push_bind(" AND es.org_id = ", org.clone())
        .push_bind(" AND es.exam_id = ", Id(exam_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok((0, None));
    };
    let count = u32::try_from(row.int(0)?.max(0)).unwrap_or(u32::MAX);
    let avg = row.opt_float(1)?.filter(|v| v.is_finite());
    Ok((count, avg))
}

async fn fetch_my_scores(
    scope: &mut Scope<'_>,
    course_id: &str,
    org: &Param,
    user_id: &str,
    exam_id: Option<&str>,
) -> Result<BTreeMap<String, MyExamScore>, StoreError> {
    let mut sql = scope.sql(
        "SELECT exam_id, score, note, updated_at \
         FROM exam_scores WHERE container_id = ",
    );
    sql.bind(Id(course_id))
        .push_bind(" AND org_id = ", org.clone())
        .push_bind(" AND user_id = ", Id(user_id));
    if let Some(eid) = exam_id {
        sql.push_bind(" AND exam_id = ", Id(eid));
    }
    let mut scores = BTreeMap::new();
    for row in scope.fetch_all(sql).await? {
        scores.insert(
            row.id(0)?,
            MyExamScore {
                score: row.float(1)?,
                note: row.text(2)?,
                updated_at: row.int(3)?,
            },
        );
    }
    Ok(scores)
}

fn stored_exam_from_row(row: &Row) -> Result<StoredExam, StoreError> {
    let results_published_at = row.opt_int(8)?;
    let published_average_score = row.opt_float(9)?.filter(|v| v.is_finite());
    let published_score_bucket = row
        .opt_int(10)?
        .map(|v| u32::try_from(v.max(0)).unwrap_or(u32::MAX));
    let published_submission_count = row
        .opt_int(11)?
        .map(|v| u32::try_from(v.max(0)).unwrap_or(u32::MAX));
    Ok(StoredExam {
        exam: CourseExam {
            id: row.id(0)?,
            container_id: row.id(1)?,
            title: row.text(2)?,
            exam_date: row.opt_text(3)?,
            max_score: row.float(4)?,
            description: row.text(5)?,
            created_at: row.int(6)?,
            updated_at: row.int(7)?,
            my_score: None,
            average_score: None,
            score_count: 0,
            results_published_at: None,
            published_score_bucket: None,
        },
        results_published_at,
        published_average_score,
        published_score_bucket,
        published_submission_count,
    })
}

/// Removes `course_exams` and `exam_scores` rows of courses that are gone for good:
/// neither live in `containers` nor awaiting restore in `deleted_containers`.
pub(crate) async fn forget_gone_containers(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "DELETE FROM course_exams WHERE NOT EXISTS \
         (SELECT 1 FROM containers AS c WHERE c.id = course_exams.container_id",
    );
    sql.changing_no_access()
        .push(&scope.same_organization(&[("c", "course_exams")]))
        .push(
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_containers AS d WHERE d.id = course_exams.container_id",
        )
        .push(&scope.same_organization(&[("d", "course_exams")]))
        .push(")");
    scope.execute(sql).await?;

    let mut sql = scope.sql(
        "DELETE FROM exam_scores WHERE NOT EXISTS \
         (SELECT 1 FROM course_exams AS e WHERE e.id = exam_scores.exam_id",
    );
    sql.changing_no_access()
        .push(&scope.same_organization(&[("e", "exam_scores")]))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Deletes every `exam_scores` row of `user_id` for courses belonging to `workspace_id`
/// (both live and in the trash).
pub(crate) async fn forget_user_in_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    course_cleanup::forget_user_in_workspace(
        scope,
        PersonalCourseTable::ExamScores,
        workspace_id,
        user_id,
    )
    .await
}

/// Deletes `exam_scores` rows of `user_id` in `workspace_id` for courses where `user_id`
/// no longer holds any direct or inherited membership (called when an org admin ends
/// their joining).
pub(crate) async fn forget_user_in_workspace_unjoined(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    course_cleanup::forget_user_in_workspace_unjoined(
        scope,
        PersonalCourseTable::ExamScores,
        workspace_id,
        user_id,
    )
    .await
}

/// Deletes every `exam_scores` row of `user_id` belonging to the scope's organization,
/// without touching scores stored in other organizations or the user's personal space.
pub(crate) async fn forget_user_in_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    course_cleanup::forget_user_in_organization(scope, PersonalCourseTable::ExamScores, user_id)
        .await
}
