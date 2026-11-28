//! Personal time tracking entries (`time_entries`) and single active timer storage.
//!
//! Rows live behind the organization wall (`org_id` + `org_wall` RLS on PostgreSQL), while
//! a partial unique index (`time_entries_one_active_per_user`) guarantees at most one
//! running timer per account across every organization. Cross-organization operations
//! (`active`, `start`, `stop`, `list`, `patch`, `delete`) run with the system's reach,
//! bind every statement strictly to `user_id = reader.user_id`, serialize per-user timer
//! transitions on the user's row (`FOR NO KEY UPDATE` on PostgreSQL, `BEGIN IMMEDIATE` on
//! SQLite), and fail closed against any organization the session does not open
//! ([`SessionTaken`]).

use fvoci_model::{
    BlockType, ContainerKind, ContainerVisibility, OrgKind, StartedTimeEntry, TimeEntry,
    TimeEntryKind, TimeEntryKindTotals, TimeEntryList, TimeEntryPatch, TimeEntrySummary,
    TimeTargetKind, WorkspaceRole, is_id, new_id,
};

use crate::Store;
use crate::blocks::Paging;
use crate::container_team_access;
use crate::db::{Dialect, FromRow, Id, OptId, Param, Row, Scope, Sql};
use crate::directory::{IN_USE, Making, SessionTaken};
use crate::error::StoreError;
use crate::snapshot::Snapshot;
use crate::users;

const TIME_ENTRY_COLUMNS: &str = "id, org_id, workspace_id, container_id, block_id, target_kind, \
     target_title, kind, started_at, ended_at, note, created_at, updated_at";

/// Single-row `SELECT` joining the user, organization, and SSO provider so session-wall
/// visibility is evaluated atomically on the target row itself (O(1), fail-closed, zero
/// TOCTOU). The trailing `p.sso_required = ` takes a `true` boolean bind.
const VISIBLE_ENTRY_SELECT: &str = "\
    SELECT te.id, te.org_id, te.workspace_id, te.container_id, te.block_id, \
           te.target_kind, te.target_title, te.kind, te.started_at, te.ended_at, \
           te.note, te.created_at, te.updated_at, o.kind, o.deleted_at, p.id, \
           CASE WHEN u.password_hash = '' THEN '' ELSE 'set' END, \
           (o.kind = 'personal' OR o.owner_id = te.user_id \
            OR EXISTS (SELECT 1 FROM organization_members AS om \
              WHERE om.org_id = te.org_id AND om.user_id = te.user_id) \
            OR EXISTS (SELECT 1 FROM workspace_members AS wm \
              JOIN workspaces AS w ON w.id = wm.workspace_id \
              WHERE w.org_id = te.org_id AND w.deleted_at IS NULL AND wm.user_id = te.user_id)) \
    FROM time_entries AS te \
    JOIN users AS u ON u.id = te.user_id \
    LEFT JOIN organizations AS o ON o.id = te.org_id \
    LEFT JOIN oidc_providers AS p ON p.org_id = te.org_id AND p.sso_required = ";

/// Divisor used to split SQLite millisecond sums into high/low chunks so `SUM()` never
/// overflows 64-bit signed integers while preserving exact millisecond totals.
const SQLITE_SUM_CHUNK: i128 = 1_000_000_000;

/// Who reads or mutates their own time entries, and with which session.
///
/// Organizations that require a specific OIDC provider (`sso_required`) and personal spaces
/// of accounts with a password (`password_sign_in_required`) are hidden from sessions that
/// do not open them ([`SessionTaken::opened_by`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeReader<'a> {
    /// The account identifier of the session.
    pub user_id: &'a str,
    /// How the session was created and whether it has confirmed the account password.
    pub making: Making<&'a str>,
}

impl TimeReader<'_> {
    /// Fail-closed allow-list of active organizations `(org_id, is_present_member)` holding
    /// at least one time entry of this user that `self.making` opens, as `snapshot` reads
    /// them: what else it reads of the entries agrees with the list.
    async fn open_organizations(
        self,
        snapshot: &mut Snapshot<'_>,
    ) -> Result<Vec<(String, bool)>, StoreError> {
        let scope = snapshot.scope();
        let mut sql = scope.sql(
            "SELECT DISTINCT te.org_id, o.kind, o.deleted_at, p.id, \
             CASE WHEN u.password_hash = '' THEN '' ELSE 'set' END, \
             (o.kind = 'personal' OR o.owner_id = u.id \
              OR EXISTS (SELECT 1 FROM organization_members AS om \
                WHERE om.org_id = te.org_id AND om.user_id = u.id) \
              OR EXISTS (SELECT 1 FROM workspace_members AS wm \
                JOIN workspaces AS w ON w.id = wm.workspace_id \
                WHERE w.org_id = te.org_id AND w.deleted_at IS NULL AND wm.user_id = u.id",
        );
        sql.push(&scope.same_organization(&[("wm", "w")]))
            .push(
                ")) FROM time_entries AS te \
                 JOIN users AS u ON u.id = te.user_id \
                 JOIN organizations AS o ON o.id = te.org_id AND o.deleted_at IS NULL \
                 LEFT JOIN oidc_providers AS p ON p.org_id = te.org_id AND p.sso_required = ",
            )
            .bind(true)
            .push_bind(" WHERE te.user_id = ", Id(self.user_id));
        let mut open = Vec::new();
        for row in scope.fetch_all(sql).await? {
            let kind = parse_org_kind(&row.text(1)?)?;
            let deleted = row.opt_int(2)?.is_some();
            let required = opt_id(&row, 3)?;
            let password = row.text(4)?;
            let present = row.bool(5)?;
            if !deleted && org_opened_by(kind, required, &password, self.making) {
                open.push((row.id(0)?, present));
            }
        }
        Ok(open)
    }
}

/// Target and metadata for creating or starting a time entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewStoredTimeEntry<'a> {
    /// Optional workspace of the target.
    pub workspace_id: Option<&'a str>,
    /// Optional board or course of the target.
    pub container_id: Option<&'a str>,
    /// Optional card or unit of the target.
    pub block_id: Option<&'a str>,
    /// Optional snapshot of the target's kind.
    pub target_kind: Option<TimeTargetKind>,
    /// Optional snapshot of the target's title.
    pub target_title: Option<&'a str>,
    /// Activity kind.
    pub kind: TimeEntryKind,
    /// User note.
    pub note: &'a str,
}

/// Filters for [`Store::list_time_entries`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeEntryFilter<'a> {
    /// Inclusive lower bound on `started_at` (epoch ms).
    pub from: Option<i64>,
    /// Inclusive upper bound on `started_at` (epoch ms).
    pub to: Option<i64>,
    /// Optional activity kind filter.
    pub kind: Option<TimeEntryKind>,
    /// Optional target block identifier filter.
    pub block_id: Option<&'a str>,
    /// Optional target container identifier filter.
    pub container_id: Option<&'a str>,
}

/// Outcome of [`Store::patch_time_entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeEntryPatchOutcome {
    /// Entry was updated and returned.
    Updated(TimeEntry),
    /// No matching entry exists for this user in an open organization.
    NotFound,
    /// Resulting `ended_at` would be earlier than `started_at`.
    EndedBeforeStarted,
}

/// Single row paired with whether the caller's session opens its organization.
struct VisibleRow {
    entry: TimeEntry,
    visible: bool,
}

impl Store {
    /// Returns the caller's currently active timer, if one is running in an organization
    /// open to the session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] or [`StoreError::Corrupt`] if the query fails.
    pub async fn active_time_entry(
        &self,
        reader: TimeReader<'_>,
    ) -> Result<Option<TimeEntry>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                Ok(select_active_with_visibility(scope, reader)
                    .await?
                    .filter(|row| row.visible)
                    .map(|row| row.entry))
            })
            .await
    }

    /// Atomically stops any currently running timer of `reader.user_id` (across all
    /// organizations) and starts a new active timer in `org_id`.
    ///
    /// If the stopped timer belonged to an organization the session does not open, it is
    /// still stopped in the database to maintain the single-active-timer invariant, while
    /// `stopped` is returned as `None` so nothing of the closed organization is revealed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::UserNotFound`] if the user does not exist or is being erased,
    /// [`StoreError::NotFound`] if the user is no longer in an active `org_id`,
    /// [`StoreError::InvalidId`] if any identifier is invalid, or [`StoreError::Database`].
    pub async fn start_time_entry(
        &self,
        reader: TimeReader<'_>,
        org_id: &str,
        new: &NewStoredTimeEntry<'_>,
        now: i64,
    ) -> Result<StartedTimeEntry, StoreError> {
        self.system()
            .transaction(async |scope| {
                users::lock_user(scope, reader.user_id).await?;
                if !user_present_in_active_org(scope, reader.user_id, org_id).await? {
                    return Err(StoreError::NotFound("target".to_owned()));
                }
                let mut stopped_visible = None;
                if let Some(mut active) = select_active_with_visibility(scope, reader).await? {
                    let stop_at = now.max(active.entry.started_at);
                    let stop_updated_at = now.max(active.entry.updated_at);
                    stop_active_row(
                        scope,
                        &active.entry.id,
                        reader.user_id,
                        stop_at,
                        stop_updated_at,
                    )
                    .await?;
                    active.entry.ended_at = Some(stop_at);
                    active.entry.updated_at = stop_updated_at;
                    if active.visible {
                        stopped_visible = Some(active.entry);
                    }
                }
                let entry = build_entry(new, now, None, now);
                insert_row(scope, org_id, reader.user_id, &entry).await?;
                Ok(StartedTimeEntry {
                    entry,
                    stopped: stopped_visible,
                })
            })
            .await
    }

    /// Stops the caller's currently running timer in an organization open to the session.
    /// Returns `None` if no visible active timer is running.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] or [`StoreError::Corrupt`] if a statement fails.
    pub async fn stop_time_entry(
        &self,
        reader: TimeReader<'_>,
        now: i64,
    ) -> Result<Option<TimeEntry>, StoreError> {
        self.system()
            .transaction(async |scope| {
                if !lock_user_if_in_use(scope, reader.user_id).await? {
                    return Ok(None);
                }
                let Some(mut active) = select_active_with_visibility(scope, reader).await? else {
                    return Ok(None);
                };
                if !active.visible {
                    return Ok(None);
                }
                let stop_at = now.max(active.entry.started_at);
                let stop_updated_at = now.max(active.entry.updated_at);
                stop_active_row(
                    scope,
                    &active.entry.id,
                    reader.user_id,
                    stop_at,
                    stop_updated_at,
                )
                .await?;
                active.entry.ended_at = Some(stop_at);
                active.entry.updated_at = stop_updated_at;
                Ok(Some(active.entry))
            })
            .await
    }

    /// Inserts a completed manual time entry through an organization-scoped handle.
    ///
    /// Does not modify any active timer. The row's `org_id` is taken from this handle's
    /// [`Tenant`](crate::Tenant), so PostgreSQL RLS enforces tenant isolation directly.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if this handle names no organization,
    /// [`StoreError::UserNotFound`] if the account is deleted/erased,
    /// [`StoreError::NotFound`] if the user is no longer in the active organization,
    /// [`StoreError::InvalidId`] if any identifier is invalid, or [`StoreError::Database`].
    pub async fn create_time_entry(
        &self,
        user_id: &str,
        new: &NewStoredTimeEntry<'_>,
        started_at: i64,
        ended_at: i64,
        now: i64,
    ) -> Result<TimeEntry, StoreError> {
        self.transaction(async |scope| {
            let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
            if !users::hold_in_use(scope, user_id).await? {
                return Err(StoreError::UserNotFound(user_id.to_owned()));
            }
            if !user_present_in_active_org(scope, user_id, &org_id).await? {
                return Err(StoreError::NotFound("target".to_owned()));
            }
            let entry = build_entry(new, started_at, Some(ended_at), now);
            insert_row(scope, &org_id, user_id, &entry).await?;
            Ok(entry)
        })
        .await
    }

    /// Lists one page of the caller's time entries across all organizations open to the
    /// session, newest first (`started_at DESC, id DESC`), together with the aggregate
    /// summary across all matching entries from the same consistent read snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] or [`StoreError::Corrupt`] if a query fails.
    pub async fn list_time_entries(
        &self,
        reader: TimeReader<'_>,
        filter: &TimeEntryFilter<'_>,
        paging: Paging,
    ) -> Result<TimeEntryList, StoreError> {
        if filter.block_id.is_some_and(|id| !is_id(id))
            || filter.container_id.is_some_and(|id| !is_id(id))
        {
            return Ok(empty_list());
        }
        self.system()
            .read_snapshot(async |snapshot| {
                let open_all = reader.open_organizations(snapshot).await?;
                let scope = snapshot.scope();
                let targets_filtered = filter.block_id.is_some() || filter.container_id.is_some();
                let mut open = Vec::with_capacity(open_all.len());
                let mut former_orgs = std::collections::HashSet::new();
                for (org_id, present) in open_all {
                    if !present {
                        if targets_filtered {
                            continue;
                        }
                        former_orgs.insert(org_id.clone());
                    }
                    open.push(org_id);
                }
                if open.is_empty() {
                    return Ok(empty_list());
                }
                let mut list_sql = scope.sql([
                    "SELECT ",
                    TIME_ENTRY_COLUMNS,
                    " FROM time_entries WHERE user_id = ",
                ]);
                push_filter(reader.user_id, &open, filter, &mut list_sql);
                list_sql
                    .push(" ORDER BY started_at DESC, id DESC")
                    .push_page(paging);
                let entries = scope
                    .fetch_all(list_sql)
                    .await?
                    .iter()
                    .map(|row| {
                        let org_id = row.id(1)?;
                        let mut entry = TimeEntry::from_row(row)?;
                        if former_orgs.contains(&org_id) {
                            mask_former_org_target(&mut entry);
                        }
                        Ok(entry)
                    })
                    .collect::<Result<Vec<_>, StoreError>>()?;

                let summary = read_summary(scope, reader.user_id, &open, filter).await?;
                Ok(TimeEntryList { entries, summary })
            })
            .await
    }

    /// Updates mutable fields (`kind`, `started_at`, `ended_at`, `note`) of one of the
    /// caller's time entries in an organization open to the session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] or [`StoreError::Corrupt`] if a statement fails.
    pub async fn patch_time_entry(
        &self,
        reader: TimeReader<'_>,
        entry_id: &str,
        patch: &TimeEntryPatch,
        now: i64,
    ) -> Result<TimeEntryPatchOutcome, StoreError> {
        if !is_id(entry_id) {
            return Ok(TimeEntryPatchOutcome::NotFound);
        }
        self.system()
            .transaction(async |scope| {
                if !lock_user_if_in_use(scope, reader.user_id).await? {
                    return Ok(TimeEntryPatchOutcome::NotFound);
                }
                let Some(found) = select_entry_with_visibility(scope, reader, entry_id).await?
                else {
                    return Ok(TimeEntryPatchOutcome::NotFound);
                };
                if !found.visible {
                    return Ok(TimeEntryPatchOutcome::NotFound);
                }
                let mut entry = found.entry;
                apply_patch(&mut entry, patch);
                if entry
                    .ended_at
                    .is_some_and(|ended_at| ended_at < entry.started_at)
                {
                    return Ok(TimeEntryPatchOutcome::EndedBeforeStarted);
                }
                entry.updated_at = now.max(entry.updated_at);
                let mut update = scope.sql("UPDATE time_entries SET kind = ");
                update
                    .bind(entry.kind.as_str())
                    .push_bind(", started_at = ", entry.started_at)
                    .push_bind(", ended_at = ", Param::OptInt(entry.ended_at))
                    .push_bind(", note = ", entry.note.as_str())
                    .push_bind(", updated_at = ", entry.updated_at)
                    .push_bind(" WHERE id = ", Id(&entry.id))
                    .push_bind(" AND user_id = ", Id(reader.user_id));
                scope.execute(update).await?;
                Ok(TimeEntryPatchOutcome::Updated(entry))
            })
            .await
    }

    /// Deletes one of the caller's time entries in an organization open to the session.
    /// Returns `true` if a row was deleted, `false` if no matching visible entry exists
    /// (including when `entry_id` is not a valid identifier).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] or [`StoreError::Corrupt`] if a statement fails.
    pub async fn delete_time_entry(
        &self,
        reader: TimeReader<'_>,
        entry_id: &str,
    ) -> Result<bool, StoreError> {
        if !is_id(entry_id) {
            return Ok(false);
        }
        self.system()
            .transaction(async |scope| {
                if !lock_user_if_in_use(scope, reader.user_id).await? {
                    return Ok(false);
                }
                let Some(found) = select_entry_with_visibility(scope, reader, entry_id).await?
                else {
                    return Ok(false);
                };
                if !found.visible {
                    return Ok(false);
                }
                let mut sql = scope.sql("DELETE FROM time_entries WHERE id = ");
                sql.bind(Id(entry_id))
                    .push_bind(" AND user_id = ", Id(reader.user_id));
                Ok(scope.execute(sql).await? > 0)
            })
            .await
    }

    /// Returns the default workspace identifier of `user_id`'s personal space, if active.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn own_space_workspace_of(
        &self,
        user_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT o.default_workspace_id FROM organizations AS o \
                     JOIN workspaces AS w ON w.id = o.default_workspace_id AND w.org_id = o.id \
                     WHERE o.owner_id = ",
                );
                sql.bind(Id(user_id))
                    .push(" AND o.deleted_at IS NULL AND w.deleted_at IS NULL");
                scope.first(sql).await
            })
            .await
    }

    /// Resolves the workspace identifier of a target `block_id` or `container_id` when the
    /// caller omitted `workspaceId`.
    ///
    /// Only returns a workspace where `user_id` is a workspace member AND has read standing
    /// on a non-template board/course (and non-template card or course unit when `block_id`
    /// is given), so an invalid or unreadable target behind an SSO wall never leaks via a
    /// 401 vs 404 oracle.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn workspace_of_time_target(
        &self,
        user_id: &str,
        container_id: Option<&str>,
        block_id: Option<&str>,
    ) -> Result<Option<String>, StoreError> {
        if container_id.is_none() && block_id.is_none() {
            return Ok(None);
        }
        if block_id.is_some_and(|id| !is_id(id)) || container_id.is_some_and(|id| !is_id(id)) {
            return Ok(None);
        }
        self.system()
            .with_conn(async |scope| {
                let mut sql = if let Some(bid) = block_id {
                    target_block_workspace_sql(scope, user_id, bid, container_id)
                } else {
                    target_container_workspace_sql(scope, user_id, container_id.unwrap_or_default())
                };
                sql.push(IN_USE)
                    .push_bind(" AND b.is_template = ", false)
                    .push_bind(" AND b.is_builtin = ", false);
                push_readable_container(&mut sql, scope, user_id);
                scope.first(sql).await
            })
            .await
    }
}

fn target_block_workspace_sql(
    scope: &Scope<'_>,
    user_id: &str,
    block_id: &str,
    container_id: Option<&str>,
) -> Sql {
    let no_card_template = match scope.dialect() {
        Dialect::Sqlite => "json_type(bl.fields, '$.isTemplate') IS NOT 'true'",
        Dialect::Postgres => "(bl.fields->'isTemplate')::text IS DISTINCT FROM 'true'",
    };
    let mut sql = scope.sql(
        "SELECT t.id FROM blocks AS bl \
         JOIN containers AS b ON b.id = bl.container_id \
         JOIN workspaces AS t ON t.id = b.workspace_id \
         JOIN organizations AS o ON o.id = t.org_id \
         JOIN workspace_members AS wm \
           ON wm.workspace_id = t.id AND wm.user_id = ",
    );
    sql.bind(Id(user_id))
        .push_bind(" WHERE bl.id = ", Id(block_id))
        .push(&scope.same_organization(&[("bl", "b"), ("b", "t"), ("wm", "t")]))
        .push_bind(" AND ((bl.type = ", BlockType::Card.as_str())
        .push_bind(" AND b.kind = ", ContainerKind::Board.as_str())
        .push(" AND ")
        .push(no_card_template)
        .push_bind(") OR (bl.type = ", BlockType::Unit.as_str())
        .push_bind(" AND b.kind = ", ContainerKind::Course.as_str())
        .push("))");
    if let Some(cid) = container_id {
        sql.push_bind(" AND b.id = ", Id(cid));
    }
    sql
}

fn target_container_workspace_sql(scope: &Scope<'_>, user_id: &str, container_id: &str) -> Sql {
    let mut sql = scope.sql(
        "SELECT t.id FROM containers AS b \
         JOIN workspaces AS t ON t.id = b.workspace_id \
         JOIN organizations AS o ON o.id = t.org_id \
         JOIN workspace_members AS wm \
           ON wm.workspace_id = t.id AND wm.user_id = ",
    );
    sql.bind(Id(user_id))
        .push_bind(" WHERE b.id = ", Id(container_id))
        .push(&scope.same_organization(&[("b", "t"), ("wm", "t")]))
        .push(" AND b.kind IN (")
        .bind_list([
            ContainerKind::Board.as_str(),
            ContainerKind::Course.as_str(),
        ])
        .push(")");
    sql
}

/// Appends the condition that `user_id` has read standing on container `b` (with workspace
/// membership `wm`).
fn push_readable_container(sql: &mut Sql, scope: &Scope<'_>, user_id: &str) {
    sql.push(
        " AND (EXISTS (SELECT 1 FROM container_members AS cm \
         WHERE cm.container_id = b.id AND cm.user_id = wm.user_id",
    )
    .push(&scope.same_organization(&[("cm", "b")]))
    .push(") OR ");
    container_team_access::push_has_team_role(sql, scope, user_id);
    sql.push_bind(" OR (b.visibility = ", ContainerVisibility::Open.as_str())
        .push(" AND b.kind IN (")
        .bind_list([
            ContainerKind::Board.as_str(),
            ContainerKind::Course.as_str(),
        ])
        .push_bind(") AND wm.role <> ", WorkspaceRole::Guest.as_str())
        .push("))");
}

/// Lock-free check (never locks `organizations` after `users`, preserving the global lock
/// order) that `org_id` is still active and `user_id` still owns or belongs to it.
async fn user_present_in_active_org(
    scope: &mut Scope<'_>,
    user_id: &str,
    org_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organizations AS o WHERE o.id = ");
    sql.bind(Id(org_id))
        .push(" AND o.deleted_at IS NULL AND (o.owner_id = ")
        .bind(Id(user_id))
        .push(
            " OR EXISTS (SELECT 1 FROM organization_members AS om WHERE om.org_id = o.id \
               AND om.user_id = ",
        )
        .bind(Id(user_id))
        .push(
            ") OR EXISTS (SELECT 1 FROM workspace_members AS wm \
               JOIN workspaces AS w ON w.id = wm.workspace_id \
               WHERE w.org_id = o.id AND w.deleted_at IS NULL AND wm.user_id = ",
        )
        .bind(Id(user_id))
        .push(&scope.same_organization(&[("wm", "w")]))
        .push("))");
    scope.exists(sql).await
}

async fn lock_user_if_in_use(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    match users::lock_user(scope, user_id).await {
        Ok(()) => Ok(true),
        Err(StoreError::UserNotFound(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn select_active_with_visibility(
    scope: &mut Scope<'_>,
    reader: TimeReader<'_>,
) -> Result<Option<VisibleRow>, StoreError> {
    let mut sql = scope.sql(VISIBLE_ENTRY_SELECT);
    sql.bind(true)
        .push_bind(" WHERE te.user_id = ", Id(reader.user_id))
        .push(" AND te.ended_at IS NULL");
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| row_to_visible(&row, reader.making))
        .transpose()
}

async fn select_entry_with_visibility(
    scope: &mut Scope<'_>,
    reader: TimeReader<'_>,
    entry_id: &str,
) -> Result<Option<VisibleRow>, StoreError> {
    let mut sql = scope.sql(VISIBLE_ENTRY_SELECT);
    sql.bind(true)
        .push_bind(" WHERE te.user_id = ", Id(reader.user_id))
        .push_bind(" AND te.id = ", Id(entry_id));
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| row_to_visible(&row, reader.making))
        .transpose()
}

async fn stop_active_row(
    scope: &mut Scope<'_>,
    entry_id: &str,
    user_id: &str,
    ended_at: i64,
    updated_at: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE time_entries SET ended_at = ");
    sql.bind(ended_at)
        .push_bind(", updated_at = ", updated_at)
        .push_bind(" WHERE id = ", Id(entry_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push(" AND ended_at IS NULL");
    scope.execute(sql).await?;
    Ok(())
}

fn apply_patch(entry: &mut TimeEntry, patch: &TimeEntryPatch) {
    if let Some(kind) = patch.kind {
        entry.kind = kind;
    }
    if let Some(started_at) = patch.started_at {
        entry.started_at = started_at;
    }
    if let Some(ended_at) = patch.ended_at {
        entry.ended_at = Some(ended_at);
    }
    if let Some(note) = &patch.note {
        entry.note.clone_from(note);
    }
}

fn build_entry(
    new: &NewStoredTimeEntry<'_>,
    started_at: i64,
    ended_at: Option<i64>,
    now: i64,
) -> TimeEntry {
    TimeEntry {
        id: new_id(),
        workspace_id: new.workspace_id.map(str::to_owned),
        container_id: new.container_id.map(str::to_owned),
        block_id: new.block_id.map(str::to_owned),
        target_kind: new.target_kind,
        target_title: new.target_title.map(str::to_owned),
        kind: new.kind,
        started_at,
        ended_at,
        note: new.note.to_owned(),
        created_at: now,
        updated_at: now,
    }
}

async fn insert_row(
    scope: &mut Scope<'_>,
    org_id: &str,
    user_id: &str,
    entry: &TimeEntry,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "INSERT INTO time_entries (id, org_id, user_id, workspace_id, container_id, block_id, \
         target_kind, target_title, kind, started_at, ended_at, note, created_at, updated_at) \
         VALUES (",
    );
    sql.bind(Id(&entry.id))
        .push_bind(", ", Id(org_id))
        .push_bind(", ", Id(user_id))
        .push_bind(", ", OptId(entry.workspace_id.as_deref().unwrap_or("")))
        .push_bind(", ", OptId(entry.container_id.as_deref().unwrap_or("")))
        .push_bind(", ", OptId(entry.block_id.as_deref().unwrap_or("")))
        .push_bind(
            ", ",
            Param::OptText(entry.target_kind.map(|kind| kind.as_str().to_owned())),
        )
        .push_bind(", ", Param::OptText(entry.target_title.clone()))
        .push_bind(", ", entry.kind.as_str())
        .push_bind(", ", entry.started_at)
        .push_bind(", ", Param::OptInt(entry.ended_at))
        .push_bind(", ", entry.note.as_str())
        .push_bind(", ", entry.created_at)
        .push_bind(", ", entry.updated_at)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

fn push_filter(user_id: &str, open: &[String], filter: &TimeEntryFilter<'_>, sql: &mut Sql) {
    sql.bind(Id(user_id))
        .push(" AND org_id IN (")
        .bind_list(open.iter().map(|org_id| Id(org_id)))
        .push(")");
    if let Some(from) = filter.from {
        sql.push_bind(" AND started_at >= ", from);
    }
    if let Some(to) = filter.to {
        sql.push_bind(" AND started_at <= ", to);
    }
    if let Some(kind) = filter.kind {
        sql.push_bind(" AND kind = ", kind.as_str());
    }
    if let Some(block_id) = filter.block_id {
        sql.push_bind(" AND block_id = ", Id(block_id));
    }
    if let Some(container_id) = filter.container_id {
        sql.push_bind(" AND container_id = ", Id(container_id));
    }
}

async fn read_summary(
    scope: &mut Scope<'_>,
    user_id: &str,
    open: &[String],
    filter: &TimeEntryFilter<'_>,
) -> Result<TimeEntrySummary, StoreError> {
    let dialect = scope.dialect();
    let select = match dialect {
        Dialect::Postgres => {
            "SELECT kind, CAST(COUNT(*) AS BIGINT), \
             CAST(LEAST(COALESCE(SUM(CASE WHEN ended_at IS NOT NULL THEN ended_at - started_at \
             ELSE 0 END), 0), 9223372036854775807) AS BIGINT) \
             FROM time_entries WHERE user_id = "
        }
        Dialect::Sqlite => {
            "SELECT kind, CAST(COUNT(*) AS BIGINT), \
             COALESCE(SUM(CASE WHEN ended_at IS NOT NULL \
             THEN (ended_at - started_at) / 1000000000 ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN ended_at IS NOT NULL \
             THEN (ended_at - started_at) % 1000000000 ELSE 0 END), 0) \
             FROM time_entries WHERE user_id = "
        }
    };
    let mut sql = scope.sql(select);
    push_filter(user_id, open, filter, &mut sql);
    sql.push(" GROUP BY kind");

    let mut by_kind = TimeEntryKindTotals::default();
    let mut count = 0_i64;
    for row in scope.fetch_all(sql).await? {
        let kind = parse_entry_kind(&row.text(0)?)?;
        count = count.saturating_add(row.int(1)?);
        let duration_ms = match dialect {
            Dialect::Postgres => row.int(2)?,
            Dialect::Sqlite => {
                let high = i128::from(row.int(2)?);
                let low = i128::from(row.int(3)?);
                i64::try_from(high * SQLITE_SUM_CHUNK + low).unwrap_or(i64::MAX)
            }
        };
        by_kind.add(kind, duration_ms);
    }
    Ok(TimeEntrySummary {
        total_ms: by_kind.total(),
        by_kind,
        count,
    })
}

fn empty_list() -> TimeEntryList {
    TimeEntryList {
        entries: Vec::new(),
        summary: TimeEntrySummary::default(),
    }
}

fn org_opened_by(
    kind: OrgKind,
    required_provider: Option<String>,
    password: &str,
    making: Making<&str>,
) -> bool {
    let takes = match kind {
        OrgKind::Personal => SessionTaken::of_space(password),
        OrgKind::Shared => SessionTaken::of_organization(required_provider),
    };
    takes.opened_by(making)
}

fn row_to_visible(row: &Row, making: Making<&str>) -> Result<VisibleRow, StoreError> {
    let mut entry = TimeEntry::from_row(row)?;
    let visible = match row.opt_text(13)? {
        Some(kind_text) => {
            let kind = parse_org_kind(&kind_text)?;
            let deleted = row.opt_int(14)?.is_some();
            let required = opt_id(row, 15)?;
            let password = row.text(16)?;
            let present = row.bool(17)?;
            if !present {
                mask_former_org_target(&mut entry);
            }
            !deleted && org_opened_by(kind, required, &password, making)
        }
        None => false,
    };
    Ok(VisibleRow { entry, visible })
}

fn mask_former_org_target(entry: &mut TimeEntry) {
    entry.workspace_id = None;
    entry.container_id = None;
    entry.block_id = None;
    entry.target_kind = None;
    entry.target_title = None;
}

impl FromRow for TimeEntry {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let target_kind = row
            .opt_text(5)?
            .map(|text| parse_target_kind(&text))
            .transpose()?;
        Ok(TimeEntry {
            id: row.id(0)?,
            workspace_id: opt_id(row, 2)?,
            container_id: opt_id(row, 3)?,
            block_id: opt_id(row, 4)?,
            target_kind,
            target_title: row.opt_text(6)?,
            kind: parse_entry_kind(&row.text(7)?)?,
            started_at: row.int(8)?,
            ended_at: row.opt_int(9)?,
            note: row.text(10)?,
            created_at: row.int(11)?,
            updated_at: row.int(12)?,
        })
    }
}

fn opt_id(row: &Row, index: usize) -> Result<Option<String>, StoreError> {
    Ok(Some(row.id(index)?).filter(|id| !id.is_empty()))
}

fn parse_entry_kind(text: &str) -> Result<TimeEntryKind, StoreError> {
    text.parse().map_err(|error| StoreError::Corrupt {
        column: "kind",
        reason: format!("{error}"),
    })
}

fn parse_target_kind(text: &str) -> Result<TimeTargetKind, StoreError> {
    text.parse().map_err(|error| StoreError::Corrupt {
        column: "target_kind",
        reason: format!("{error}"),
    })
}

fn parse_org_kind(text: &str) -> Result<OrgKind, StoreError> {
    text.parse().map_err(|error| StoreError::Corrupt {
        column: "kind",
        reason: format!("{error}"),
    })
}
