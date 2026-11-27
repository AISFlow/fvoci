//! Lifecycle cleanup of `unit_reviews` and `unit_notes` on permanent purge,
//! workspace removal, org-admin unjoining, and organization departure.

use crate::course_cleanup::{self, PersonalCourseTable, UnitCourseTable};
use crate::db::Scope;
use crate::error::StoreError;

/// Both personal learning tables cleaned up on purge and membership removal.
const TABLES: [PersonalCourseTable; 2] = [
    PersonalCourseTable::UnitReviews,
    PersonalCourseTable::UnitNotes,
];

const UNIT_TABLES: [UnitCourseTable; 2] = [UnitCourseTable::Reviews, UnitCourseTable::Notes];

/// Forgets personal review and note rows of units that are gone for good: neither live in
/// `blocks` nor awaiting restore in `deleted_blocks` of the row's organization.
pub(crate) async fn forget_gone_blocks(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    for table in UNIT_TABLES {
        course_cleanup::forget_gone_unit_rows(scope, table).await?;
    }
    Ok(())
}

/// Deletes every `unit_reviews` and `unit_notes` row of `user_id` for courses belonging
/// to `workspace_id` (both live and in the trash).
pub(crate) async fn forget_user_in_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    for table in TABLES {
        course_cleanup::forget_user_in_workspace(scope, table, workspace_id, user_id).await?;
    }
    Ok(())
}

/// Deletes `unit_reviews` and `unit_notes` rows of `user_id` in `workspace_id` for
/// courses where `user_id` no longer holds any direct or inherited membership.
pub(crate) async fn forget_user_in_workspace_unjoined(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    for table in TABLES {
        course_cleanup::forget_user_in_workspace_unjoined(scope, table, workspace_id, user_id)
            .await?;
    }
    Ok(())
}

/// Deletes every `unit_reviews` and `unit_notes` row of `user_id` belonging to the
/// scope's organization, without touching other organizations or personal spaces.
pub(crate) async fn forget_user_in_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    for table in TABLES {
        course_cleanup::forget_user_in_organization(scope, table, user_id).await?;
    }
    Ok(())
}
