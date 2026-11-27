//! Personal study notes (`unit_notes`) on course units.

use fvoci_model::{
    BlockType, ContainerKind, NewUnitNote, UnitNote, UnitNoteChange, UnitNoteKind, is_id, new_id,
};

use crate::Store;
use crate::db::{Id, Param, Row, Scope};
use crate::error::StoreError;
use crate::unit_progress::require_live_unit;

impl Store {
    /// Lists the calling learner's personal notes on live unit `unit_id` of `course_id`,
    /// ordered by `created_at DESC, id DESC`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `unit_id` is not a live unit of `course_id`, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn list_my_unit_notes(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        kind: Option<UnitNoteKind>,
    ) -> Result<Vec<UnitNote>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            require_live_unit_exists(scope, course_id, unit_id, org.clone()).await?;
            let mut sql = scope.sql(
                "SELECT id, unit_id, container_id, kind, content, created_at, updated_at \
                 FROM unit_notes \
                 WHERE container_id = ",
            );
            sql.bind(Id(course_id))
                .push_bind(" AND unit_id = ", Id(unit_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND org_id = ", org);
            if let Some(kind) = kind {
                sql.push_bind(" AND kind = ", kind.as_str());
            }
            sql.push(" ORDER BY created_at DESC, id DESC");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(note_from_row)
                .collect()
        })
        .await
    }

    /// Creates a personal note on live unit `unit_id` of `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `unit_id` is not a live unit of `course_id` or the
    /// user is not an active workspace member, and [`StoreError::Database`] if a
    /// statement fails.
    pub async fn create_unit_note(
        &self,
        course_id: &str,
        unit_id: &str,
        user_id: &str,
        new: &NewUnitNote,
    ) -> Result<UnitNote, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            require_live_unit(scope, course_id, unit_id, user_id, org.clone()).await?;
            let id = new_id();
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO unit_notes \
                 (id, org_id, unit_id, container_id, user_id, kind, content, created_at, \
                  updated_at) VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(&id))
                .push_bind(", ", org)
                .push_bind(", ", Id(unit_id))
                .push_bind(", ", Id(course_id))
                .push_bind(", ", Id(user_id))
                .push_bind(", ", new.kind.as_str())
                .push_bind(", ", new.content.as_str())
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push(
                    ") RETURNING id, unit_id, container_id, kind, content, created_at, updated_at",
                );
            let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
            note_from_row(&row)
        })
        .await
    }

    /// Updates the caller's personal note `note_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `user_id` is not an identifier,
    /// [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if `note_id` does not belong to `user_id` in `course_id`
    /// or its unit is no longer live, and [`StoreError::Database`] if a statement fails.
    pub async fn patch_unit_note(
        &self,
        course_id: &str,
        note_id: &str,
        user_id: &str,
        change: &UnitNoteChange,
    ) -> Result<UnitNote, StoreError> {
        if !is_id(user_id) {
            return Err(StoreError::InvalidId(user_id.to_owned()));
        }
        if !is_id(note_id) {
            return Err(StoreError::not_found("unit note"));
        }
        self.transaction(async |scope| {
            let org = scope.org()?;
            let unit_id =
                find_live_note_unit(scope, course_id, note_id, user_id, org.clone()).await?;
            // Acquire `blocks -> workspace_members -> users` before locking `unit_notes`
            // so concurrent removal/erasure cannot deadlock on PostgreSQL.
            require_live_unit(scope, course_id, &unit_id, user_id, org.clone()).await?;
            let now = scope.now_millis();
            let mut sql = scope.sql("UPDATE unit_notes SET ");
            sql.changing_no_access();
            if let Some(kind) = change.kind {
                sql.push_bind("kind = ", kind.as_str()).push(", ");
            }
            if let Some(content) = change.content.as_deref() {
                sql.push_bind("content = ", content).push(", ");
            }
            sql.push_bind("updated_at = ", now)
                .push_bind(" WHERE id = ", Id(note_id))
                .push_bind(" AND container_id = ", Id(course_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND org_id = ", org)
                .push(
                    " RETURNING id, unit_id, container_id, kind, content, created_at, updated_at",
                );
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(|| StoreError::not_found("unit note"))?;
            note_from_row(&row)
        })
        .await
    }

    /// Deletes the caller's personal note `note_id` in `course_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::NotFound`] if no such note belongs to `user_id` in `course_id`, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn delete_unit_note(
        &self,
        course_id: &str,
        note_id: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        if !is_id(note_id) {
            return Err(StoreError::not_found("unit note"));
        }
        self.with_write_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql("DELETE FROM unit_notes WHERE id = ");
            sql.changing_no_access()
                .bind(Id(note_id))
                .push_bind(" AND container_id = ", Id(course_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND org_id = ", org);
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("unit note"));
            }
            Ok(())
        })
        .await
    }
}

async fn require_live_unit_exists(
    scope: &mut Scope<'_>,
    course_id: &str,
    unit_id: &str,
    org: Param,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT 1 FROM blocks AS b \
         JOIN containers AS c ON c.id = b.container_id \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE b.id = ",
    );
    sql.bind(Id(unit_id))
        .push_bind(" AND b.container_id = ", Id(course_id))
        .push_bind(" AND b.type = ", BlockType::Unit.as_str())
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("b", "c"), ("c", "w")]));
    if scope.fetch_optional(sql).await?.is_none() {
        return Err(StoreError::not_found("unit"));
    }
    Ok(())
}

async fn find_live_note_unit(
    scope: &mut Scope<'_>,
    course_id: &str,
    note_id: &str,
    user_id: &str,
    org: Param,
) -> Result<String, StoreError> {
    let mut sql = scope.sql(
        "SELECT un.unit_id \
         FROM unit_notes AS un \
         JOIN blocks AS b ON b.id = un.unit_id \
         JOIN containers AS c ON c.id = b.container_id \
         JOIN workspaces AS w ON w.id = c.workspace_id \
         JOIN organizations AS o ON o.id = w.org_id \
         WHERE un.id = ",
    );
    sql.bind(Id(note_id))
        .push_bind(" AND un.container_id = ", Id(course_id))
        .push_bind(" AND un.user_id = ", Id(user_id))
        .push_bind(" AND un.org_id = ", org.clone())
        .push(" AND b.container_id = un.container_id")
        .push_bind(" AND b.type = ", BlockType::Unit.as_str())
        .push_bind(" AND c.kind = ", ContainerKind::Course.as_str())
        .push_bind(" AND w.org_id = ", org)
        .push(" AND w.deleted_at IS NULL")
        .push(" AND o.deleted_at IS NULL")
        .push(&scope.same_organization(&[("un", "b"), ("b", "c"), ("c", "w")]));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("unit note"))?;
    row.id(0)
}

fn note_from_row(row: &Row) -> Result<UnitNote, StoreError> {
    Ok(UnitNote {
        id: row.id(0)?,
        unit_id: row.id(1)?,
        container_id: row.id(2)?,
        kind: note_kind_from_row(row, 3)?,
        content: row.text(4)?,
        created_at: row.int(5)?,
        updated_at: row.int(6)?,
    })
}

fn note_kind_from_row(row: &Row, index: usize) -> Result<UnitNoteKind, StoreError> {
    row.text(index)?
        .parse()
        .map_err(
            |error: fvoci_model::ParseUnitNoteKindError| StoreError::Corrupt {
                column: "kind",
                reason: error.to_string(),
            },
        )
}
