//! Course study meetings (`course_meetings`), shared attendance (`meeting_attendance`),
//! and upcoming-meeting notifications (`meeting_upcoming`).
//!
//! Meetings and attendance stay while a course is in the trash so restoration preserves
//! them, and are removed when the course is permanently purged ([`forget_gone_containers`]).
//! Attendance rows are intentionally kept as tombstones when an account is erased.

use std::collections::{BTreeMap, BTreeSet};

use fvoci_model::{
    AttendanceStatus, ContainerKind, CourseMeeting, CourseMeetingChange, MeetingAttendance,
    NotificationKind, OptionalIdChange, OrgKind, ValidatedNewMeeting, is_id, new_id,
};

use crate::Store;
use crate::containers::inherited_members_source;
use crate::db::{Dialect, Id, OptId, Param, Row, Scope, Sql, SqlFragment};
use crate::error::StoreError;
use crate::notification_outbox;

/// How many identifiers one batched `IN (...)` statement binds.
const BATCH: usize = 300;

/// Maximum notifications kept per recipient, matching `app::notifications::LIMITS`.
const MOST_NOTIFICATIONS_OF_A_PERSON: i64 = 10_000;

/// Location and title of a live course meeting for resolving `meeting_upcoming`
/// notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingNotificationPlace {
    /// Identifier of the meeting.
    pub meeting_id: String,
    /// Current meeting title.
    pub title: String,
    /// Course container identifier.
    pub container_id: String,
    /// Workspace holding the course.
    pub workspace_id: String,
}

/// One recipient notified of an upcoming course meeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingNotified {
    /// Recipient user identifier.
    pub recipient_id: String,
    /// Organization identifier of the course.
    pub org_id: String,
}

impl Store {
    /// Lists all study meetings of live course `course_id` in chronological order
    /// (`starts_at ASC, id ASC`), each with its attendance records ordered by `user_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if a query fails.
    pub async fn list_course_meetings(
        &self,
        course_id: &str,
    ) -> Result<Vec<CourseMeeting>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql(
                "SELECT m.id, m.container_id, m.title, m.starts_at, m.location, m.unit_ids, \
                 m.presenter_id, m.host_id, m.notes, m.notified_at, m.created_at, m.updated_at \
                 FROM course_meetings AS m \
                 JOIN containers AS c ON c.id = m.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 WHERE m.container_id = ",
            );
            sql.bind(Id(course_id))
                .push_bind(" AND m.org_id = ", org.clone())
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND w.org_id = ", org.clone())
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("m", "c"), ("c", "w")]))
                .push(" ORDER BY m.starts_at ASC, m.id ASC");
            let rows = scope.fetch_all(sql).await?;
            let mut meetings = Vec::with_capacity(rows.len());
            for row in &rows {
                meetings.push(meeting_from_row(row)?);
            }
            if meetings.is_empty() {
                return Ok(meetings);
            }
            let ids: Vec<&str> = meetings.iter().map(|m| m.id.as_str()).collect();
            let mut by_meeting = fetch_attendance_map(scope, &org, &ids).await?;
            for meeting in &mut meetings {
                if let Some(records) = by_meeting.remove(&meeting.id) {
                    meeting.attendance = records;
                }
            }
            Ok(meetings)
        })
        .await
    }

    /// Creates a new study meeting on live course `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `course_id` is not a live course or if an assigned
    /// `presenter_id` / `host_id` is not an active enrolled member of the course, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn create_course_meeting(
        &self,
        course_id: &str,
        new: &ValidatedNewMeeting,
    ) -> Result<CourseMeeting, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            for member_id in [new.presenter_id.as_deref(), new.host_id.as_deref()]
                .into_iter()
                .flatten()
            {
                require_active_course_member(scope, course_id, member_id, org.clone()).await?;
            }
            let id = new_id();
            let now = scope.now_millis();
            let unit_ids_json =
                serde_json::to_string(&new.unit_ids).map_err(|error| StoreError::Corrupt {
                    column: "unit_ids",
                    reason: error.to_string(),
                })?;
            let mut sql = scope.sql(
                "INSERT INTO course_meetings \
                 (id, org_id, container_id, title, starts_at, location, unit_ids, \
                  presenter_id, host_id, notes, notified_at, created_at, updated_at) VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(&id))
                .push_bind(", ", org)
                .push_bind(", ", Id(course_id))
                .push_bind(", ", &new.title)
                .push_bind(", ", new.starts_at)
                .push_bind(", ", &new.location)
                .push_bind(", ", unit_ids_json)
                .push_bind(", ", OptId(new.presenter_id.as_deref().unwrap_or("")))
                .push_bind(", ", OptId(new.host_id.as_deref().unwrap_or("")))
                .push_bind(", ", &new.notes)
                .push_bind(", ", Param::OptInt(None))
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push(")");
            scope.execute(sql).await?;
            Ok(CourseMeeting {
                id,
                container_id: course_id.to_owned(),
                title: new.title.clone(),
                starts_at: new.starts_at,
                location: new.location.clone(),
                unit_ids: new.unit_ids.clone(),
                presenter_id: new.presenter_id.clone(),
                host_id: new.host_id.clone(),
                notes: new.notes.clone(),
                notified_at: None,
                created_at: now,
                updated_at: now,
                attendance: Vec::new(),
            })
        })
        .await
    }

    /// Updates fields of a study meeting in live course `course_id` and returns the
    /// updated meeting with its attendance records.
    ///
    /// Resets `notified_at` to `NULL` only when `starts_at` changes to a different time.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or meeting does not exist or a newly
    /// assigned `presenter_id` / `host_id` is not an active course member, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn patch_course_meeting(
        &self,
        course_id: &str,
        meeting_id: &str,
        change: &CourseMeetingChange,
    ) -> Result<CourseMeeting, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let existing = fetch_meeting_for_update(scope, course_id, meeting_id, &org).await?;
            for member_id in [&change.presenter_id, &change.host_id] {
                if let OptionalIdChange::Set(uid) = member_id {
                    require_active_course_member(scope, course_id, uid, org.clone()).await?;
                }
            }
            let now = scope.now_millis();
            let title = change.title.as_deref().unwrap_or(&existing.title);
            let starts_at = change.starts_at.unwrap_or(existing.starts_at);
            let location = change.location.as_deref().unwrap_or(&existing.location);
            let unit_ids = change.unit_ids.as_ref().unwrap_or(&existing.unit_ids);
            let presenter_id =
                apply_id_change(existing.presenter_id.as_deref(), &change.presenter_id);
            let host_id = apply_id_change(existing.host_id.as_deref(), &change.host_id);
            let notes = change.notes.as_deref().unwrap_or(&existing.notes);
            let notified_at = if starts_at == existing.starts_at {
                existing.notified_at
            } else {
                None
            };
            let unit_ids_json =
                serde_json::to_string(unit_ids).map_err(|error| StoreError::Corrupt {
                    column: "unit_ids",
                    reason: error.to_string(),
                })?;
            let mut sql = scope.sql("UPDATE course_meetings SET title = ");
            sql.changing_no_access()
                .bind(title)
                .push_bind(", starts_at = ", starts_at)
                .push_bind(", location = ", location)
                .push_bind(", unit_ids = ", unit_ids_json)
                .push_bind(
                    ", presenter_id = ",
                    OptId(presenter_id.as_deref().unwrap_or("")),
                )
                .push_bind(", host_id = ", OptId(host_id.as_deref().unwrap_or("")))
                .push_bind(", notes = ", notes)
                .push_bind(", notified_at = ", Param::OptInt(notified_at))
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(meeting_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            scope.execute(sql).await?;
            let attendance = fetch_attendance_map(scope, &org, &[meeting_id])
                .await?
                .remove(meeting_id)
                .unwrap_or_default();
            Ok(CourseMeeting {
                id: existing.id,
                container_id: existing.container_id,
                title: title.to_owned(),
                starts_at,
                location: location.to_owned(),
                unit_ids: unit_ids.clone(),
                presenter_id,
                host_id,
                notes: notes.to_owned(),
                notified_at,
                created_at: existing.created_at,
                updated_at: now,
                attendance,
            })
        })
        .await
    }

    /// Permanently deletes a study meeting of live course `course_id`, along with its
    /// attendance rows and any `meeting_upcoming` notifications pointing to it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or meeting does not exist, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn delete_course_meeting(
        &self,
        course_id: &str,
        meeting_id: &str,
    ) -> Result<(), StoreError> {
        if !is_id(meeting_id) {
            return Err(StoreError::not_found("meeting"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut sql = scope.sql("DELETE FROM course_meetings WHERE id = ");
            sql.changing_no_access()
                .bind(Id(meeting_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("meeting"));
            }
            let mut sql = scope.sql("DELETE FROM meeting_attendance WHERE meeting_id = ");
            sql.changing_no_access()
                .bind(Id(meeting_id))
                .push_bind(" AND org_id = ", org.clone());
            scope.execute(sql).await?;
            let mut sql = scope.sql("DELETE FROM notifications WHERE container_id = ");
            sql.changing_no_access()
                .bind(Id(course_id))
                .push_bind(" AND org_id = ", org)
                .push_bind(" AND block_id = ", Id(meeting_id))
                .push_bind(" AND kind = ", NotificationKind::MeetingUpcoming.as_str());
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Records or updates `user_id`'s attendance on `meeting_id` of live course
    /// `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if the course or meeting does not exist or `user_id` is
    /// not an active enrolled member of `course_id`, and [`StoreError::Database`] if a
    /// statement fails.
    pub async fn upsert_meeting_attendance(
        &self,
        course_id: &str,
        meeting_id: &str,
        user_id: &str,
        status: AttendanceStatus,
    ) -> Result<MeetingAttendance, StoreError> {
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_course(scope, course_id, org.clone()).await?;
            let mut exists = scope.sql("SELECT 1 FROM course_meetings WHERE id = ");
            exists
                .bind(Id(meeting_id))
                .push_bind(" AND org_id = ", org.clone())
                .push_bind(" AND container_id = ", Id(course_id));
            if scope.dialect() == Dialect::Postgres {
                exists.push(" FOR KEY SHARE");
            }
            if scope.fetch_optional(exists).await?.is_none() {
                return Err(StoreError::not_found("meeting"));
            }
            require_active_course_member(scope, course_id, user_id, org.clone()).await?;
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO meeting_attendance (meeting_id, org_id, user_id, status, updated_at) \
                 VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(meeting_id))
                .push_bind(", ", org)
                .push_bind(", ", Id(user_id))
                .push_bind(", ", status.as_str())
                .push_bind(", ", now)
                .push(
                    ") ON CONFLICT (meeting_id, org_id, user_id) \
                     DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at",
                );
            scope.execute(sql).await?;
            Ok(MeetingAttendance {
                user_id: user_id.to_owned(),
                status,
                updated_at: now,
            })
        })
        .await
    }

    /// Finds unnotified meetings starting within `[now, now + window_millis]` in live
    /// non-template courses of live shared organizations, marks them notified once
    /// (`notified_at = now`), and inserts one `meeting_upcoming` notification per active
    /// enrolled course member.
    ///
    /// Operates within the handle's tenant scope (called via `App::housekeeping()` to sweep
    /// every organization).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if any statement fails.
    pub async fn notify_upcoming_meetings(
        &self,
        now: i64,
        window_millis: i64,
    ) -> Result<Vec<MeetingNotified>, StoreError> {
        let until = now.saturating_add(window_millis.max(0));
        self.transaction(async |scope| {
            let mut claim = scope.sql("UPDATE course_meetings SET notified_at = ");
            claim
                .changing_no_access()
                .bind(now)
                .push(" WHERE notified_at IS NULL")
                .push_bind(" AND starts_at >= ", now)
                .push_bind(" AND starts_at <= ", until)
                .push(
                    " AND EXISTS (SELECT 1 FROM containers AS c \
                         JOIN workspaces AS w ON w.id = c.workspace_id \
                         JOIN organizations AS o ON o.id = w.org_id \
                         WHERE c.id = course_meetings.container_id",
                )
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
                .push(&scope.same_organization(&[("course_meetings", "c"), ("c", "w")]))
                .push(") RETURNING id, org_id, container_id");
            let claimed: Vec<(String, String, String)> = scope
                .fetch_all(claim)
                .await?
                .iter()
                .map(|row| Ok((row.id(0)?, row.id(1)?, row.id(2)?)))
                .collect::<Result<_, StoreError>>()?;
            let mut sent = Vec::new();
            for (meeting_id, org_id, course_id) in claimed {
                let members = active_course_member_ids(scope, &course_id, &org_id).await?;
                for user_id in members {
                    insert_meeting_notification(
                        scope,
                        &org_id,
                        &user_id,
                        &meeting_id,
                        &course_id,
                        now,
                    )
                    .await?;
                    sent.push(MeetingNotified {
                        recipient_id: user_id,
                        org_id: org_id.clone(),
                    });
                }
            }
            Ok(sent)
        })
        .await
    }

    /// Resolves live course meetings among `meeting_ids` visible to this org-scoped
    /// handle, for rendering `meeting_upcoming` notifications.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn meeting_notification_places(
        &self,
        meeting_ids: &[&str],
    ) -> Result<Vec<MeetingNotificationPlace>, StoreError> {
        let ids: BTreeSet<&str> = meeting_ids.iter().copied().collect();
        let ids: Vec<&str> = ids.into_iter().collect();
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut places = Vec::with_capacity(ids.len());
            for batch in ids.chunks(BATCH) {
                let mut sql = scope.sql(
                    "SELECT m.id, m.title, m.container_id, c.workspace_id \
                     FROM course_meetings AS m \
                     JOIN containers AS c ON c.id = m.container_id \
                     JOIN workspaces AS w ON w.id = c.workspace_id \
                     JOIN organizations AS o ON o.id = w.org_id \
                     WHERE c.kind = ",
                );
                sql.bind(ContainerKind::Course.as_str())
                    .push_bind(" AND m.org_id = ", org.clone())
                    .push_bind(" AND w.org_id = ", org.clone())
                    .push_bind(" AND c.is_template = ", false)
                    .push(" AND w.deleted_at IS NULL")
                    .push(" AND o.deleted_at IS NULL")
                    .push(&scope.same_organization(&[("m", "c"), ("c", "w")]))
                    .push(" AND m.id IN (")
                    .bind_list(batch.iter().map(|id| Id(id)))
                    .push(")");
                for row in scope.fetch_all(sql).await? {
                    places.push(MeetingNotificationPlace {
                        meeting_id: row.id(0)?,
                        title: row.text(1)?,
                        container_id: row.id(2)?,
                        workspace_id: row.id(3)?,
                    });
                }
            }
            Ok(places)
        })
        .await
    }
}

/// Verifies `course_id` is a live `ContainerKind::Course` in a live workspace and
/// organization matching `org`, returning its `workspace_id`.
pub(crate) async fn require_live_course(
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
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE OF c");
    }
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Err(StoreError::not_found("course"));
    };
    row.id(0)
}

/// Appends the SQL subquery producing active enrolled `user_id` rows for live course
/// `course_id` in organization `org` (both direct `container_members` and inherited group
/// members who are active workspace members and not deleted/erased).
pub(crate) fn push_active_course_members_subquery(
    scope: &Scope<'_>,
    sql: &mut Sql,
    course_id: &str,
    org: Param,
) {
    let inherited = inherited_members_source(scope);
    let course_kind = ContainerKind::Course.as_str();
    sql.push(
        "(SELECT cm.user_id \
          FROM container_members AS cm \
          JOIN containers AS c ON c.id = cm.container_id \
          JOIN workspace_members AS wm \
            ON wm.workspace_id = c.workspace_id AND wm.user_id = cm.user_id \
          JOIN workspaces AS w ON w.id = c.workspace_id \
          JOIN organizations AS o ON o.id = w.org_id \
          JOIN users AS u ON u.id = cm.user_id \
          WHERE cm.container_id = ",
    )
    .bind(Id(course_id))
    .push_bind(" AND c.kind = ", course_kind)
    .push_bind(" AND w.org_id = ", org.clone())
    .push(" AND w.deleted_at IS NULL")
    .push(" AND o.deleted_at IS NULL")
    .push(" AND u.deleted_at IS NULL")
    .push_bind(" AND u.erased_at = ", 0_i64)
    .push(&scope.same_organization(&[("cm", "c"), ("wm", "c"), ("w", "c")]))
    .push(" UNION SELECT im.user_id FROM ")
    .push(&inherited)
    .push(
        " AS im \
          JOIN containers AS c ON c.id = im.container_id \
          JOIN workspaces AS w ON w.id = c.workspace_id \
          JOIN users AS u ON u.id = im.user_id \
          WHERE im.container_id = ",
    )
    .bind(Id(course_id))
    .push_bind(" AND c.kind = ", course_kind)
    .push_bind(" AND im.org_id = ", org)
    .push(" AND w.deleted_at IS NULL")
    .push(" AND u.deleted_at IS NULL")
    .push_bind(" AND u.erased_at = ", 0_i64)
    .push(&scope.same_organization(&[("im", "c"), ("c", "w")]))
    .push(")");
}

pub(crate) async fn require_active_course_member(
    scope: &mut Scope<'_>,
    course_id: &str,
    user_id: &str,
    org: Param,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM ");
    push_active_course_members_subquery(scope, &mut sql, course_id, org);
    sql.push_bind(" AS am WHERE am.user_id = ", Id(user_id));
    if scope.fetch_optional(sql).await?.is_none() {
        return Err(StoreError::not_found("member"));
    }
    Ok(())
}

async fn active_course_member_ids(
    scope: &mut Scope<'_>,
    course_id: &str,
    org_id: &str,
) -> Result<Vec<String>, StoreError> {
    let org = Param::Id(Some(
        sqlx::types::Uuid::try_parse(org_id)
            .map_err(|_| StoreError::InvalidId(org_id.to_owned()))?,
    ));
    let mut sql = scope.sql("SELECT am.user_id FROM ");
    push_active_course_members_subquery(scope, &mut sql, course_id, org);
    sql.push(" AS am ORDER BY am.user_id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.id(0))
        .collect()
}

async fn insert_meeting_notification(
    scope: &mut Scope<'_>,
    org_id: &str,
    recipient_id: &str,
    meeting_id: &str,
    course_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let id = new_id();
    let mut sql = scope.sql(
        "INSERT INTO notifications (recipient_id, id, org_id, kind, source_id, \
         block_id, container_id, actor_id, made_by, created_at, updated_at, times, \
         doers, counted, read_at) VALUES (",
    );
    sql.changing_no_access()
        .bind(Id(recipient_id))
        .push_bind(", ", Id(&id))
        .push_bind(", ", Id(org_id))
        .push_bind(", ", NotificationKind::MeetingUpcoming.as_str())
        .push_bind(", ", Id(meeting_id))
        .push_bind(", ", Id(meeting_id))
        .push_bind(", ", Id(course_id))
        .push_bind(", ", OptId(""))
        .push_bind(", ", OptId(""))
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(", 1, '[]', '[]', 0)");
    scope.execute(sql).await?;
    notification_outbox::keep_newest(scope, recipient_id, MOST_NOTIFICATIONS_OF_A_PERSON).await
}

async fn fetch_meeting_for_update(
    scope: &mut Scope<'_>,
    course_id: &str,
    meeting_id: &str,
    org: &Param,
) -> Result<CourseMeeting, StoreError> {
    let mut sql = scope.sql(
        "SELECT id, container_id, title, starts_at, location, unit_ids, presenter_id, \
         host_id, notes, notified_at, created_at, updated_at \
         FROM course_meetings WHERE id = ",
    );
    sql.bind(Id(meeting_id))
        .push_bind(" AND org_id = ", org.clone())
        .push_bind(" AND container_id = ", Id(course_id));
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Err(StoreError::not_found("meeting"));
    };
    meeting_from_row(&row)
}

async fn fetch_attendance_map(
    scope: &mut Scope<'_>,
    org: &Param,
    meeting_ids: &[&str],
) -> Result<BTreeMap<String, Vec<MeetingAttendance>>, StoreError> {
    let mut map: BTreeMap<String, Vec<MeetingAttendance>> = BTreeMap::new();
    for batch in meeting_ids.chunks(BATCH) {
        let mut sql = scope.sql(
            "SELECT meeting_id, user_id, status, updated_at \
             FROM meeting_attendance WHERE org_id = ",
        );
        sql.bind(org.clone())
            .push(" AND meeting_id IN (")
            .bind_list(batch.iter().map(|id| Id(id)))
            .push(") ORDER BY meeting_id ASC, user_id ASC");
        for row in scope.fetch_all(sql).await? {
            let meeting_id = row.id(0)?;
            let status: AttendanceStatus =
                row.text(2)?
                    .parse()
                    .map_err(
                        |error: fvoci_model::UnknownAttendanceStatus| StoreError::Corrupt {
                            column: "status",
                            reason: error.to_string(),
                        },
                    )?;
            map.entry(meeting_id).or_default().push(MeetingAttendance {
                user_id: row.id(1)?,
                status,
                updated_at: row.int(3)?,
            });
        }
    }
    Ok(map)
}

fn meeting_from_row(row: &Row) -> Result<CourseMeeting, StoreError> {
    let unit_ids_text = row.text(5)?;
    let unit_ids: Vec<String> =
        serde_json::from_str(&unit_ids_text).map_err(|error| StoreError::Corrupt {
            column: "unit_ids",
            reason: error.to_string(),
        })?;
    let presenter_id = non_empty(row.id(6)?);
    let host_id = non_empty(row.id(7)?);
    Ok(CourseMeeting {
        id: row.id(0)?,
        container_id: row.id(1)?,
        title: row.text(2)?,
        starts_at: row.int(3)?,
        location: row.text(4)?,
        unit_ids,
        presenter_id,
        host_id,
        notes: row.text(8)?,
        notified_at: row.opt_int(9)?,
        created_at: row.int(10)?,
        updated_at: row.int(11)?,
        attendance: Vec::new(),
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn apply_id_change(existing: Option<&str>, change: &OptionalIdChange) -> Option<String> {
    match change {
        OptionalIdChange::Keep => existing.map(str::to_owned),
        OptionalIdChange::Clear => None,
        OptionalIdChange::Set(id) => Some(id.clone()),
    }
}

/// Removes `meeting_attendance` and `course_meetings` rows of courses that are gone for
/// good: neither live in `containers` nor awaiting restore in `deleted_containers`.
pub(crate) async fn forget_gone_containers(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut del_meetings = SqlFragment::of(&["DELETE FROM course_meetings WHERE NOT EXISTS \
         (SELECT 1 FROM containers AS c WHERE c.id = course_meetings.container_id"]);
    del_meetings
        .push_fragment(&scope.same_organization(&[("c", "course_meetings")]))
        .push(
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_containers AS d WHERE d.id = course_meetings.container_id",
        )
        .push_fragment(&scope.same_organization(&[("d", "course_meetings")]))
        .push(")");
    let mut sql = scope.sql(&del_meetings);
    sql.changing_no_access();
    scope.execute(sql).await?;

    let mut del_attendance = SqlFragment::of(&["DELETE FROM meeting_attendance WHERE NOT EXISTS \
         (SELECT 1 FROM course_meetings AS m WHERE m.id = meeting_attendance.meeting_id"]);
    del_attendance
        .push_fragment(&scope.same_organization(&[("m", "meeting_attendance")]))
        .push(")");
    let mut sql = scope.sql(&del_attendance);
    sql.changing_no_access();
    scope.execute(sql).await?;
    Ok(())
}
