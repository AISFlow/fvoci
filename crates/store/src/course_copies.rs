//! Copying a course whole (`duplicateContainer` for `ContainerKind::Course`).
//!
//! Copies the course container, its live chapters and units (preserving hierarchy,
//! sibling order, bodies, study `amount`/`amountUnit`, and shared `scheduledDate`),
//! remaps internal `[[` body references to the copied units, and copies the course's
//! shared schedule (`course_schedules`, or a built-in course template's example schedule
//! shifted to the copier's day).
//!
//! Personal learner data (`unit_progress`: completion, `last_position`, personal
//! `planned_date`), comments, trashed units, body versions, subscriptions, notifications,
//! other members, access groups, and public links are never copied.

use fvoci_model::{
    BlockError, BlockType, Container, ContainerKind, ContainerMember, ContainerVisibility,
    ContainersAndBlocks, CourseLimits, CourseScheduleUpdate, TEMPLATE_SCHEDULE_PROPERTY,
    ValidatedCourseSchedule, renew_containers_and_blocks_ids, shift_unit_scheduled_date,
};

use crate::blocks::{BlockFilter, CopyChanges, CopyTitles};
use crate::containers_and_blocks::{NewContainers, create_whole};
use crate::db::Scope;
use crate::file_refs::NewBodyFiles;
use crate::{Store, StoreError, blocks, containers, course_schedules, workspace_membership};
use fvoci_model::DuplicateAs;

impl Store {
    /// Copies a course (or course template) with its live chapters, units, study amounts,
    /// shared unit dates, and shared `course_schedules` under fresh identifiers.
    ///
    /// Built-in course templates are read without taking a write lock (compatible with
    /// PostgreSQL read-only template RLS), have their template marks stripped via `changes`,
    /// and have both their example schedule and unit `scheduledDate` values shifted by
    /// `changes.days_later` days.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such course or `to_workspace` is
    /// outside this organization, [`StoreError::InvalidBlock`] if the course exceeds
    /// [`CourseLimits`], and [`StoreError::Database`] if a statement fails.
    pub async fn duplicate_course(
        &self,
        course_id: &str,
        user_id: &str,
        to_workspace: Option<&str>,
        duplicate_as: DuplicateAs,
        titles: &CopyTitles,
        changes: CopyChanges<'_>,
    ) -> Result<(ContainersAndBlocks, Vec<ContainerMember>), StoreError> {
        self.transaction(async |scope| {
            duplicate_course(
                scope,
                course_id,
                user_id,
                to_workspace,
                duplicate_as,
                titles,
                changes,
            )
            .await
        })
        .await
    }
}

async fn duplicate_course(
    scope: &mut Scope<'_>,
    course_id: &str,
    user_id: &str,
    to_workspace: Option<&str>,
    duplicate_as: DuplicateAs,
    titles: &CopyTitles,
    changes: CopyChanges<'_>,
) -> Result<(ContainersAndBlocks, Vec<ContainerMember>), StoreError> {
    let mut container = load_source_course(scope, course_id).await?;
    let (schedule, shift_days) = match read_source_schedule(scope, &container).await? {
        Some(source_schedule) => {
            if let Some(shifted) = source_schedule.try_shifted(changes.days_later) {
                (Some(shifted), changes.days_later)
            } else {
                tracing::warn!(
                    course_id = %container.id,
                    days_later = changes.days_later,
                    "course schedule could not be shifted cleanly; keeping original template dates"
                );
                (Some(source_schedule), 0)
            }
        }
        None => (None, changes.days_later),
    };

    for name in changes.without_properties {
        container.properties.remove(*name);
    }

    let as_template = duplicate_as == DuplicateAs::Template;
    if as_template == container.is_template {
        container.title = (titles.copy_of)(&container.title);
    } else if as_template {
        titles.new_template.clone_into(&mut container.title);
    }

    container.visibility = ContainerVisibility::Private;
    container.is_template = as_template;
    user_id.clone_into(&mut container.created_by);
    if container.is_builtin && to_workspace.is_none() {
        return Err(StoreError::not_found("workspace"));
    }
    if let Some(workspace_id) = to_workspace {
        if !workspace_membership::is_workspace_here(scope, workspace_id).await? {
            return Err(StoreError::not_found("workspace"));
        }
        workspace_id.clone_into(&mut container.workspace_id);
    }

    let mut blocks: Vec<_> = blocks::get_blocks(scope, &BlockFilter::container(course_id))
        .await?
        .into_iter()
        .filter(|block| matches!(block.block_type, BlockType::Chapter | BlockType::Unit))
        .collect();
    check_course_block_limits(course_id, &blocks, CourseLimits::DEFAULT)?;
    if shift_days != 0 {
        for block in &mut blocks {
            if block.block_type == BlockType::Unit {
                shift_unit_scheduled_date(&mut block.fields, shift_days);
            }
        }
    }

    let mut copies = renew_containers_and_blocks_ids(ContainersAndBlocks {
        containers: vec![container],
        blocks,
    });
    let made = NewContainers::whole(&mut copies).body_files(NewBodyFiles::Unchecked);
    let members = create_whole(scope, made, user_id).await?;

    if let (Some(schedule), Some(new_course)) = (schedule, copies.containers.first()) {
        let org = scope.org()?;
        course_schedules::upsert_schedule_row(scope, &new_course.id, &schedule, org).await?;
    }

    Ok((copies, members))
}

/// Loads the source course container, taking a row lock only for non-builtin courses
/// because PostgreSQL RLS grants SELECT-only access to built-in system templates.
async fn load_source_course(
    scope: &mut Scope<'_>,
    course_id: &str,
) -> Result<Container, StoreError> {
    let not_found = || StoreError::not_found(format_args!("course ID={course_id}"));
    let container = containers::get_container(scope, course_id).await?;
    if container.kind != ContainerKind::Course {
        return Err(not_found());
    }
    if container.is_builtin {
        return Ok(container);
    }
    if containers::lock_container_of_kind(scope, course_id).await? != Some(ContainerKind::Course) {
        return Err(not_found());
    }
    containers::get_container(scope, course_id).await
}

/// Reads the shared schedule to copy: from `templateSchedule` on built-in course
/// templates (avoiding cross-org reads on `course_schedules`), or from `course_schedules`
/// for courses owned by this organization.
async fn read_source_schedule(
    scope: &mut Scope<'_>,
    source: &Container,
) -> Result<Option<ValidatedCourseSchedule>, StoreError> {
    if source.is_builtin {
        let Some(raw) = source.properties.get(TEMPLATE_SCHEDULE_PROPERTY) else {
            return Ok(None);
        };
        let update: CourseScheduleUpdate = match serde_json::from_value(raw.clone()) {
            Ok(update) => update,
            Err(error) => {
                tracing::warn!(
                    course_id = %source.id,
                    %error,
                    "ignoring malformed templateSchedule on built-in course template"
                );
                return Ok(None);
            }
        };
        return match update.check_request() {
            Ok(validated) => Ok(Some(validated)),
            Err(error) => {
                tracing::warn!(
                    course_id = %source.id,
                    %error,
                    "ignoring invalid templateSchedule on built-in course template"
                );
                Ok(None)
            }
        };
    }
    let org = scope.org()?;
    let stored = course_schedules::fetch_course_schedule(scope, &source.id, org).await?;
    Ok(stored.and_then(|schedule| {
        let validated = schedule.to_validated();
        if validated.is_none() {
            tracing::warn!(
                course_id = %source.id,
                "ignoring malformed stored course_schedules row during duplication"
            );
        }
        validated
    }))
}

fn check_course_block_limits(
    course_id: &str,
    blocks: &[fvoci_model::Block],
    limits: CourseLimits,
) -> Result<(), StoreError> {
    let chapters = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Chapter)
        .count();
    if usize::try_from(limits.chapters).is_ok_and(|max| chapters > max) {
        return Err(StoreError::InvalidBlock {
            id: course_id.to_owned(),
            source: BlockError::TooManyChapters {
                limit: limits.chapters,
            },
        });
    }
    let units = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Unit)
        .count();
    if usize::try_from(limits.units).is_ok_and(|max| units > max) {
        return Err(StoreError::InvalidBlock {
            id: course_id.to_owned(),
            source: BlockError::TooManyUnits {
                limit: limits.units,
            },
        });
    }
    Ok(())
}
