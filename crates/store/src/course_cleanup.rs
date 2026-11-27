//! Shared lifecycle cleanup for personal course learning tables (`unit_progress`,
//! `unit_reviews`, `unit_notes`, and `exam_scores`) on permanent purge, workspace
//! removal, org-admin unjoining, and organization departure.
//!
//! Every delete statement binds `org_id` unconditionally on both PostgreSQL and SQLite so
//! coexisting personal spaces and shared organizations on SQLite remain strictly isolated.

use crate::containers::inherited_members_source;
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;

/// Personal course table targeted by a lifecycle cleanup operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersonalCourseTable {
    UnitProgress,
    UnitReviews,
    UnitNotes,
    ExamScores,
}

impl PersonalCourseTable {
    const fn name(self) -> &'static str {
        match self {
            Self::UnitProgress => "unit_progress",
            Self::UnitReviews => "unit_reviews",
            Self::UnitNotes => "unit_notes",
            Self::ExamScores => "exam_scores",
        }
    }

    const fn delete_where_user(self) -> &'static str {
        match self {
            Self::UnitProgress => "DELETE FROM unit_progress WHERE user_id = ",
            Self::UnitReviews => "DELETE FROM unit_reviews WHERE user_id = ",
            Self::UnitNotes => "DELETE FROM unit_notes WHERE user_id = ",
            Self::ExamScores => "DELETE FROM exam_scores WHERE user_id = ",
        }
    }

    const fn not_exists_direct_member(self) -> &'static str {
        match self {
            Self::UnitProgress => {
                ") AND NOT EXISTS (\
                 SELECT 1 FROM container_members AS cm \
                 WHERE cm.container_id = unit_progress.container_id \
                   AND cm.user_id = unit_progress.user_id"
            }
            Self::UnitReviews => {
                ") AND NOT EXISTS (\
                 SELECT 1 FROM container_members AS cm \
                 WHERE cm.container_id = unit_reviews.container_id \
                   AND cm.user_id = unit_reviews.user_id"
            }
            Self::UnitNotes => {
                ") AND NOT EXISTS (\
                 SELECT 1 FROM container_members AS cm \
                 WHERE cm.container_id = unit_notes.container_id \
                   AND cm.user_id = unit_notes.user_id"
            }
            Self::ExamScores => {
                ") AND NOT EXISTS (\
                 SELECT 1 FROM container_members AS cm \
                 WHERE cm.container_id = exam_scores.container_id \
                   AND cm.user_id = exam_scores.user_id"
            }
        }
    }

    const fn inherited_join_where(self) -> &'static str {
        match self {
            Self::UnitProgress => {
                " AS im \
                 WHERE im.container_id = unit_progress.container_id \
                   AND im.user_id = unit_progress.user_id"
            }
            Self::UnitReviews => {
                " AS im \
                 WHERE im.container_id = unit_reviews.container_id \
                   AND im.user_id = unit_reviews.user_id"
            }
            Self::UnitNotes => {
                " AS im \
                 WHERE im.container_id = unit_notes.container_id \
                   AND im.user_id = unit_notes.user_id"
            }
            Self::ExamScores => {
                " AS im \
                 WHERE im.container_id = exam_scores.container_id \
                   AND im.user_id = exam_scores.user_id"
            }
        }
    }
}

/// Unit-scoped personal learning table targeted by gone-block cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnitCourseTable {
    Progress,
    Reviews,
    Notes,
}

impl UnitCourseTable {
    const fn name(self) -> &'static str {
        match self {
            Self::Progress => "unit_progress",
            Self::Reviews => "unit_reviews",
            Self::Notes => "unit_notes",
        }
    }
}

/// Forgets personal unit rows of units that are gone for good: neither live in `blocks`
/// nor awaiting restore in `deleted_blocks` of the row's organization.
pub(crate) async fn forget_gone_unit_rows(
    scope: &mut Scope<'_>,
    table: UnitCourseTable,
) -> Result<(), StoreError> {
    let name = table.name();
    let (head, mid) = match table {
        UnitCourseTable::Progress => (
            "DELETE FROM unit_progress WHERE NOT EXISTS \
             (SELECT 1 FROM blocks AS b WHERE b.id = unit_progress.unit_id",
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_blocks AS d WHERE d.id = unit_progress.unit_id",
        ),
        UnitCourseTable::Reviews => (
            "DELETE FROM unit_reviews WHERE NOT EXISTS \
             (SELECT 1 FROM blocks AS b WHERE b.id = unit_reviews.unit_id",
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_blocks AS d WHERE d.id = unit_reviews.unit_id",
        ),
        UnitCourseTable::Notes => (
            "DELETE FROM unit_notes WHERE NOT EXISTS \
             (SELECT 1 FROM blocks AS b WHERE b.id = unit_notes.unit_id",
            ") AND NOT EXISTS \
             (SELECT 1 FROM deleted_blocks AS d WHERE d.id = unit_notes.unit_id",
        ),
    };
    let mut sql = scope.sql(head);
    sql.changing_no_access()
        .push(&scope.same_organization(&[("b", name)]))
        .push(mid)
        .push(&scope.same_organization(&[("d", name)]))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Deletes every row in `table` for `user_id` belonging to courses in `workspace_id`
/// (both live and in the trash) within the scope's organization.
pub(crate) async fn forget_user_in_workspace(
    scope: &mut Scope<'_>,
    table: PersonalCourseTable,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let org = scope.org()?;
    let mut sql = scope.sql(table.delete_where_user());
    sql.changing_no_access()
        .bind(Id(user_id))
        .push_bind(" AND org_id = ", org)
        .push_bind(
            " AND container_id IN (\
             SELECT id FROM containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push_bind(
            " UNION SELECT id FROM deleted_containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Deletes rows in `table` for `user_id` in `workspace_id` for courses where `user_id`
/// no longer holds any direct or inherited membership within the scope's organization.
pub(crate) async fn forget_user_in_workspace_unjoined(
    scope: &mut Scope<'_>,
    table: PersonalCourseTable,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let org = scope.org()?;
    let name = table.name();
    let inherited = inherited_members_source(scope);
    let mut sql = scope.sql(table.delete_where_user());
    sql.changing_no_access()
        .bind(Id(user_id))
        .push_bind(" AND org_id = ", org.clone())
        .push_bind(
            " AND container_id IN (\
             SELECT id FROM containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push_bind(
            " UNION SELECT id FROM deleted_containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push(table.not_exists_direct_member())
        .push(&scope.same_organization(&[("cm", name)]))
        .push(") AND NOT EXISTS (SELECT 1 FROM ")
        .push(&inherited)
        .push(table.inherited_join_where())
        .push_bind(" AND im.org_id = ", org)
        .push(&scope.same_organization(&[("im", name)]))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Deletes every row in `table` of `user_id` belonging to the scope's organization,
/// without touching rows stored in other organizations or personal spaces.
pub(crate) async fn forget_user_in_organization(
    scope: &mut Scope<'_>,
    table: PersonalCourseTable,
    user_id: &str,
) -> Result<(), StoreError> {
    let org = scope.org()?;
    let mut sql = scope.sql(table.delete_where_user());
    sql.changing_no_access().bind(Id(user_id));
    match scope.dialect() {
        Dialect::Postgres => {
            sql.push_bind(" AND org_id = ", org);
        }
        Dialect::Sqlite => {
            sql.push_bind(" AND (org_id = ", org.clone())
                .push_bind(
                    " OR (org_id = '' AND container_id IN (\
                     SELECT c.id FROM containers AS c \
                     JOIN workspaces AS w ON w.id = c.workspace_id WHERE w.org_id = ",
                    org.clone(),
                )
                .push_bind(
                    " UNION SELECT d.id FROM deleted_containers AS d \
                     JOIN workspaces AS w ON w.id = d.workspace_id WHERE w.org_id = ",
                    org,
                )
                .push(")))");
        }
    }
    scope.execute(sql).await?;
    Ok(())
}
