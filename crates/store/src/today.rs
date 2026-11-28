//! Workspace and account-wide `'Today'` queries (`workspace_today`) plus background
//! `due_soon` card notification candidates and idempotent delivery (`card_due_notices`).

use fvoci_model::{
    BlockType, ContainerKind, ContainerVisibility, DiaryDate, NotificationKind,
    TODAY_SECTION_LIMIT, TodayCardItem, TodayMeetingItem, TodayReviewEntry, TodayUnitEntry,
    TodayWorkspaceMeta, WorkspaceRole, WorkspaceToday, days_between, new_id,
};

use crate::Store;
use crate::card_numbers;
use crate::containers::inherited_members_source;
use crate::db::{Id, Row};
use crate::error::StoreError;
use crate::notification_outbox::keep_newest;

/// Candidate card assignment due soon or today inspected by background housekeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSoonCardCandidate {
    /// Organization owning the board.
    pub org_id: String,
    /// Workspace owning the board.
    pub workspace_id: String,
    /// Board container identifier.
    pub board_id: String,
    /// Card block identifier.
    pub card_id: String,
    /// Assigned user identifier.
    pub user_id: String,
    /// Indexed `YYYY-MM-DD` date (`blocks.due_at`).
    pub due_at: String,
    /// Exact due timestamp in milliseconds (`blocks.due_millis`), if the property holds
    /// a timestamp rather than a calendar date.
    pub due_millis: Option<i64>,
    /// User's configured time-zone setting (`"auto"` or an IANA zone name).
    pub user_time_zone: String,
}

impl DueSoonCardCandidate {
    /// Canonical deduplication key stored in `card_due_notices.due_key`.
    #[must_use]
    pub fn due_key(&self) -> String {
        match self.due_millis {
            Some(ms) => format!("t:{ms}"),
            None => format!("d:{}", self.due_at),
        }
    }
}

/// Time window parameters for evaluating `'Today'` in a workspace.
pub struct TodayWindow<'a, F> {
    /// Caller's current calendar day (`YYYY-MM-DD`).
    pub today: DiaryDate,
    /// Upper bound calendar day (`YYYY-MM-DD`) for due-soon cards (e.g. `today + 1 day`).
    pub soon_date: &'a str,
    /// Upper bound SQL `due_at` (`YYYY-MM-DD`) accounting for UTC+14/UTC-12 offset on
    /// timestamp due dates (`soon_date + 1 day`).
    pub sql_max_due_at: &'a str,
    /// Current epoch milliseconds (lower bound for upcoming meetings).
    pub now_millis: i64,
    /// Upper bound epoch milliseconds for upcoming meetings (e.g. `now + 48h`).
    pub meetings_until_millis: i64,
    /// Converts a timestamp `due_millis` into the caller's local `YYYY-MM-DD` date.
    pub day_of_millis: F,
}

impl<F> std::fmt::Debug for TodayWindow<'_, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TodayWindow")
            .field("today", &self.today)
            .field("soon_date", &self.soon_date)
            .field("sql_max_due_at", &self.sql_max_due_at)
            .field("now_millis", &self.now_millis)
            .field("meetings_until_millis", &self.meetings_until_millis)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Computes the `'Today'` summary for `user_id` inside `workspace_id` on an
    /// organization-scoped handle.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if any query fails.
    pub async fn workspace_today<F>(
        &self,
        workspace_id: &str,
        workspace: TodayWorkspaceMeta,
        user_id: &str,
        window: &TodayWindow<'_, F>,
    ) -> Result<WorkspaceToday, StoreError>
    where
        F: Fn(i64) -> Option<String>,
    {
        let today_str = window.today.to_string();
        let (cards, has_more_cards) = self
            .workspace_today_cards(workspace_id, &workspace, user_id, &today_str, window)
            .await?;

        let learning = self
            .get_workspace_learning_today(workspace_id, user_id, window.today)
            .await?;
        let mut all_units =
            Vec::with_capacity(learning.today_units.len() + learning.overdue_units.len());
        all_units.extend(learning.overdue_units);
        all_units.extend(learning.today_units);
        let has_more_units = all_units.len() > TODAY_SECTION_LIMIT;
        all_units.truncate(TODAY_SECTION_LIMIT);
        let units = all_units
            .into_iter()
            .map(|unit| TodayUnitEntry {
                unit,
                workspace: workspace.clone(),
            })
            .collect();

        let mut all_reviews = self
            .list_workspace_today_reviews(workspace_id, user_id, &today_str)
            .await?;
        let has_more_reviews = all_reviews.len() > TODAY_SECTION_LIMIT;
        all_reviews.truncate(TODAY_SECTION_LIMIT);
        let reviews = all_reviews
            .into_iter()
            .map(|review| TodayReviewEntry {
                review,
                workspace: workspace.clone(),
            })
            .collect();

        let (meetings, has_more_meetings) = self
            .workspace_today_meetings(
                workspace_id,
                &workspace,
                user_id,
                window.now_millis,
                window.meetings_until_millis,
            )
            .await?;

        Ok(WorkspaceToday {
            today: today_str,
            workspace,
            cards,
            has_more_cards,
            units,
            has_more_units,
            reviews,
            has_more_reviews,
            meetings,
            has_more_meetings,
        })
    }

    async fn workspace_today_cards<F>(
        &self,
        workspace_id: &str,
        workspace: &TodayWorkspaceMeta,
        user_id: &str,
        today_str: &str,
        window: &TodayWindow<'_, F>,
    ) -> Result<(Vec<TodayCardItem>, bool), StoreError>
    where
        F: Fn(i64) -> Option<String>,
    {
        let fetch_limit = i64::try_from(TODAY_SECTION_LIMIT + 32).unwrap_or(132);
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let inherited = inherited_members_source(scope);
            let mut sql = scope.sql(
                "SELECT b.id, b.number, b.title, b.fields, c.id, c.title, c.icon, \
                 b.due_at, b.due_millis \
                 FROM card_assignees AS ca \
                 JOIN blocks AS b ON b.id = ca.card_id AND b.container_id = ca.container_id \
                 JOIN containers AS c ON c.id = ca.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 JOIN workspace_members AS wm \
                   ON wm.workspace_id = w.id AND wm.user_id = ca.user_id \
                 WHERE ca.org_id = ",
            );
            sql.bind(org.clone())
                .push_bind(" AND ca.user_id = ", Id(user_id))
                .push_bind(" AND c.workspace_id = ", Id(workspace_id))
                .push_bind(" AND c.kind = ", ContainerKind::Board.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push_bind(" AND b.type = ", BlockType::Card.as_str())
                .push_bind(" AND b.is_done = ", false)
                .push(" AND b.due_at IS NOT NULL")
                .push_bind(" AND b.due_at <= ", window.sql_max_due_at)
                .push_bind(" AND w.org_id = ", org.clone())
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("ca", "b"), ("b", "c"), ("c", "w"), ("wm", "w")]))
                .push(" AND ((wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push_bind(" AND c.visibility = ", ContainerVisibility::Open.as_str())
                .push(
                    ") OR EXISTS (SELECT 1 FROM container_members AS cm \
                     WHERE cm.container_id = c.id AND cm.user_id = ca.user_id",
                )
                .push(&scope.same_organization(&[("cm", "c")]))
                .push(") OR (wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push(" AND EXISTS (SELECT 1 FROM ")
                .push(&inherited)
                .push(" AS im WHERE im.container_id = c.id AND im.user_id = ca.user_id")
                .push_bind(" AND im.org_id = ", org)
                .push(&scope.same_organization(&[("im", "c")]))
                .push("))) ORDER BY b.due_at ASC, b.created_at ASC, b.id ASC LIMIT ")
                .bind(fetch_limit);

            let mut matched = Vec::new();
            for row in scope.fetch_all(sql).await? {
                if let Some(item) = today_card_from_row(
                    &row,
                    workspace,
                    today_str,
                    window.soon_date,
                    &window.day_of_millis,
                )? {
                    matched.push(item);
                }
            }
            matched.sort_by(|a, b| {
                a.due_date
                    .cmp(&b.due_date)
                    .then_with(|| a.card_id.cmp(&b.card_id))
            });
            let has_more = matched.len() > TODAY_SECTION_LIMIT;
            matched.truncate(TODAY_SECTION_LIMIT);
            Ok((matched, has_more))
        })
        .await
    }

    async fn workspace_today_meetings(
        &self,
        workspace_id: &str,
        workspace: &TodayWorkspaceMeta,
        user_id: &str,
        now_millis: i64,
        until_millis: i64,
    ) -> Result<(Vec<TodayMeetingItem>, bool), StoreError> {
        let fetch_limit = i64::try_from(TODAY_SECTION_LIMIT + 1).unwrap_or(101);
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let inherited = inherited_members_source(scope);
            let mut sql = scope.sql(
                "SELECT m.id, m.container_id, c.title, m.title, m.starts_at, m.location \
                 FROM course_meetings AS m \
                 JOIN containers AS c ON c.id = m.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 JOIN workspace_members AS wm \
                   ON wm.workspace_id = w.id AND wm.user_id = ",
            );
            sql.bind(Id(user_id))
                .push_bind(" WHERE m.org_id = ", org.clone())
                .push_bind(" AND c.workspace_id = ", Id(workspace_id))
                .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push_bind(" AND m.starts_at >= ", now_millis)
                .push_bind(" AND m.starts_at <= ", until_millis)
                .push_bind(" AND w.org_id = ", org.clone())
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(&scope.same_organization(&[("m", "c"), ("c", "w"), ("wm", "w")]))
                .push(
                    " AND (EXISTS (SELECT 1 FROM container_members AS cm \
                     WHERE cm.container_id = c.id AND cm.user_id = wm.user_id",
                )
                .push(&scope.same_organization(&[("cm", "c")]))
                .push(") OR (wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push(" AND EXISTS (SELECT 1 FROM ")
                .push(&inherited)
                .push(" AS im WHERE im.container_id = c.id AND im.user_id = wm.user_id")
                .push_bind(" AND im.org_id = ", org)
                .push(&scope.same_organization(&[("im", "c")]))
                .push("))) ORDER BY m.starts_at ASC, m.id ASC LIMIT ")
                .bind(fetch_limit);

            let mut meetings = Vec::new();
            for row in scope.fetch_all(sql).await? {
                meetings.push(TodayMeetingItem {
                    meeting_id: row.id(0)?,
                    container_id: row.id(1)?,
                    course_title: row.text(2)?,
                    title: row.text(3)?,
                    starts_at: row.int(4)?,
                    ends_at: None,
                    location: row.text(5)?,
                    workspace: workspace.clone(),
                });
            }
            let has_more = meetings.len() > TODAY_SECTION_LIMIT;
            meetings.truncate(TODAY_SECTION_LIMIT);
            Ok((meetings, has_more))
        })
        .await
    }

    /// Lists up to `limit` un-notified incomplete card assignments whose `due_at <=
    /// max_due_at` across all live shared AND personal organizations reachable by this
    /// handle, filtering out users who disabled `notify_due` (`user_settings.notify_due =
    /// FALSE`), in the order of `(due_at, card_id, user_id)`: those after `after` when it
    /// is given, so that a caller who passes over some goes on to the next.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn due_soon_card_candidates(
        &self,
        max_due_at: &str,
        after: Option<(&str, &str, &str)>,
        limit: i64,
    ) -> Result<Vec<DueSoonCardCandidate>, StoreError> {
        self.with_conn(async |scope| {
            let inherited = inherited_members_source(scope);
            let mut sql = scope.sql(
                "SELECT w.org_id, c.workspace_id, c.id, b.id, ca.user_id, b.due_at, b.due_millis, \
                 COALESCE(us.time_zone, 'auto') \
                 FROM card_assignees AS ca \
                 JOIN blocks AS b ON b.id = ca.card_id AND b.container_id = ca.container_id \
                 JOIN containers AS c ON c.id = ca.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 JOIN organizations AS o ON o.id = w.org_id \
                 JOIN users AS u ON u.id = ca.user_id \
                 JOIN workspace_members AS wm \
                   ON wm.workspace_id = w.id AND wm.user_id = ca.user_id \
                 LEFT JOIN user_settings AS us ON us.user_id = u.id \
                 WHERE ca.org_id = w.org_id \
                   AND b.type = ",
            );
            sql.bind(BlockType::Card.as_str())
                .push_bind(" AND b.is_done = ", false)
                .push(" AND b.due_at IS NOT NULL")
                .push_bind(" AND b.due_at <= ", max_due_at)
                .push_bind(" AND c.kind = ", ContainerKind::Board.as_str())
                .push_bind(" AND c.is_template = ", false)
                .push(" AND w.deleted_at IS NULL")
                .push(" AND o.deleted_at IS NULL")
                .push(" AND u.deleted_at IS NULL")
                .push_bind(" AND u.erased_at = ", 0_i64)
                .push_bind(" AND COALESCE(us.notify_due, ", true)
                .push_bind(") = ", true)
                .push(&scope.same_organization(&[("ca", "b"), ("b", "c"), ("c", "w"), ("wm", "w")]))
                .push(" AND ((wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push_bind(" AND c.visibility = ", ContainerVisibility::Open.as_str())
                .push(
                    ") OR EXISTS (SELECT 1 FROM container_members AS cm \
                     WHERE cm.container_id = c.id AND cm.user_id = ca.user_id",
                )
                .push(&scope.same_organization(&[("cm", "c")]))
                .push(") OR (wm.role <> ")
                .bind(WorkspaceRole::Guest.as_str())
                .push(" AND EXISTS (SELECT 1 FROM ")
                .push(&inherited)
                .push(
                    " AS im WHERE im.container_id = c.id AND im.user_id = ca.user_id \
                     AND im.org_id = w.org_id",
                )
                .push(&scope.same_organization(&[("im", "c")]))
                .push(
                    "))) AND NOT EXISTS (\
                     SELECT 1 FROM card_due_notices AS dn \
                     WHERE dn.org_id = w.org_id AND dn.card_id = b.id \
                       AND dn.user_id = ca.user_id \
                       AND dn.due_key = CASE \
                         WHEN b.due_millis IS NOT NULL THEN 't:' || CAST(b.due_millis AS TEXT) \
                         ELSE 'd:' || b.due_at END",
                )
                .push(&scope.same_organization(&[("dn", "b")]))
                .push(")");
            if let Some((due_at, card_id, user_id)) = after {
                sql.push_bind(" AND (b.due_at > ", due_at)
                    .push_bind(" OR (b.due_at = ", due_at)
                    .push_bind(" AND (b.id > ", Id(card_id))
                    .push_bind(" OR (b.id = ", Id(card_id))
                    .push_bind(" AND ca.user_id > ", Id(user_id))
                    .push("))))");
            }
            sql.push_bind(
                " ORDER BY b.due_at ASC, b.id ASC, ca.user_id ASC LIMIT ",
                limit,
            );

            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(DueSoonCardCandidate {
                        org_id: row.id(0)?,
                        workspace_id: row.id(1)?,
                        board_id: row.id(2)?,
                        card_id: row.id(3)?,
                        user_id: row.id(4)?,
                        due_at: row.text(5)?,
                        due_millis: row.opt_int(6)?,
                        user_time_zone: row.text(7)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Atomically records `(org_id, card_id, user_id, due_key)` in `card_due_notices` and
    /// inserts a `due_soon` notification for `candidate.user_id` (both in shared
    /// organizations AND in `OrgKind::Personal` spaces). Returns `false` if already
    /// notified or if the user disabled `notify_due`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn notify_due_soon_card(
        &self,
        candidate: &DueSoonCardCandidate,
        now: i64,
        most_of_a_person: i64,
    ) -> Result<bool, StoreError> {
        let due_key = candidate.due_key();
        self.transaction(async |scope| {
            let mut pref_sql = scope.sql("SELECT notify_due FROM user_settings WHERE user_id = ");
            pref_sql.bind(Id(&candidate.user_id));
            if let Some(row) = scope.fetch_optional(pref_sql).await?
                && !row.bool(0)?
            {
                return Ok(false);
            }

            let mut still_due = scope.sql(
                "SELECT 1 FROM blocks AS b \
                 JOIN card_assignees AS ca \
                   ON ca.card_id = b.id AND ca.container_id = b.container_id \
                 WHERE b.id = ",
            );
            still_due
                .bind(Id(&candidate.card_id))
                .push_bind(" AND b.container_id = ", Id(&candidate.board_id))
                .push_bind(" AND ca.user_id = ", Id(&candidate.user_id))
                .push_bind(" AND ca.org_id = ", Id(&candidate.org_id))
                .push_bind(" AND b.is_done = ", false)
                .push(" AND b.due_at IS NOT NULL")
                .push(&scope.same_organization(&[("ca", "b")]));
            if !scope.exists(still_due).await? {
                return Ok(false);
            }

            let mut claim = scope.sql(
                "INSERT INTO card_due_notices (org_id, card_id, user_id, due_key, notified_at) \
                 VALUES (",
            );
            claim
                .changing_no_access()
                .bind(Id(&candidate.org_id))
                .push_bind(", ", Id(&candidate.card_id))
                .push_bind(", ", Id(&candidate.user_id))
                .push_bind(", ", due_key)
                .push_bind(", ", now)
                .push(") ON CONFLICT (org_id, card_id, user_id, due_key) DO NOTHING");
            if scope.execute(claim).await? == 0 {
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
                .push_bind(", ", NotificationKind::DueSoon.as_str())
                .push_bind(", ", Id(&candidate.card_id))
                .push_bind(", ", Id(&candidate.card_id))
                .push_bind(", ", Id(&candidate.board_id))
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

fn today_card_from_row<F>(
    row: &Row,
    workspace: &TodayWorkspaceMeta,
    today_str: &str,
    soon_date: &str,
    day_of_millis: &F,
) -> Result<Option<TodayCardItem>, StoreError>
where
    F: Fn(i64) -> Option<String>,
{
    let fields = row.json_map(3, "fields")?;
    if fields.get("isTemplate") == Some(&serde_json::Value::Bool(true)) {
        return Ok(None);
    }
    let stored_due_at = row.text(7)?;
    let due_at_millis = row.opt_int(8)?;
    let due_date = match due_at_millis {
        Some(ms) => day_of_millis(ms).unwrap_or(stored_due_at),
        None => stored_due_at,
    };
    if due_date.as_str() > soon_date {
        return Ok(None);
    }
    let delta = days_between(&due_date, today_str).unwrap_or(0);
    let overdue_days = i32::try_from(delta).unwrap_or(if delta > 0 { i32::MAX } else { i32::MIN });
    let icon = fields
        .get("icon")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(Some(TodayCardItem {
        card_id: row.id(0)?,
        card_number: card_numbers::number_from_row(row, 1)?,
        title: row.text(2)?,
        icon,
        board_id: row.id(4)?,
        board_title: row.text(5)?,
        board_icon: row.text(6)?,
        due_date,
        due_at_millis,
        overdue_days,
        workspace: workspace.clone(),
    }))
}
