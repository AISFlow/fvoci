//! A person's stars on the boards, wikis, courses, cards, pages and units of a
//! workspace (`favorites`): theirs alone, in their order, a hundred a workspace.
//!
//! A star holds the identifier of what it is on and no key to it: whether its person
//! reads that is the server's to ask whenever the stars are listed, and what they do
//! not read is left out. A star goes when what it is on goes for good
//! ([`forget_gone_targets`]), when its person leaves the workspace or the organization,
//! and when their account is erased; it stays while what it is on is in the trash, and
//! shows again once that comes back.

use fvoci_model::{FavoriteKind, MAX_FAVORITES};

use crate::Store;
use crate::db::{FromRow, Id, LockRank, Row, RowLock, Scope, SqlFragment};
use crate::error::StoreError;

/// A star as it is stored: what it is on, and nothing of what that is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFavorite {
    /// The identifier of what the star is on.
    pub target_id: String,
    /// What that was when the star was put on it.
    pub kind: FavoriteKind,
}

/// What came of putting a star on something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FavoriteAdded {
    /// The star is new, after the others.
    Added,
    /// It was there already, and stays where it was.
    AlreadyThere,
    /// The person has [`MAX_FAVORITES`] stars in the workspace already: nothing changed.
    Full,
}

impl FromRow for StoredFavorite {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let kind = row
            .text(1)?
            .parse()
            .map_err(
                |error: fvoci_model::UnknownFavoriteKind| StoreError::Corrupt {
                    column: "target_kind",
                    reason: error.to_string(),
                },
            )?;
        Ok(Self {
            target_id: row.id(0)?,
            kind,
        })
    }
}

impl Store {
    /// The stars of `user_id` in the workspace, in their order: by `sort_order`, those of
    /// one order the oldest first, then by what they are on.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// for a kind this server does not know.
    pub async fn favorites_of(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<StoredFavorite>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql =
                scope.sql("SELECT target_id, target_kind FROM favorites WHERE user_id = ");
            sql.bind(Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id))
                .push(" ORDER BY sort_order, created_at, target_id");
            scope.all(sql).await
        })
        .await
    }

    /// Puts a star of `user_id` on `target_id` in the workspace, after their others; one
    /// that is there stays as it is. Past [`MAX_FAVORITES`] in the workspace nothing is
    /// put. What the target is, and whether the user reads it, is for the caller to have
    /// asked. Two put at once are put one after the other (`hold_stars_of`), so that
    /// neither is put twice nor past the limit.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is put then.
    pub async fn add_favorite(
        &self,
        workspace_id: &str,
        user_id: &str,
        favorite: &StoredFavorite,
        now: i64,
    ) -> Result<FavoriteAdded, StoreError> {
        self.transaction(async |scope| {
            hold_stars_of(scope, workspace_id, user_id).await?;
            let mut sql = scope.sql("SELECT workspace_id FROM favorites WHERE user_id = ");
            sql.bind(Id(user_id))
                .push_bind(" AND target_id = ", Id(&favorite.target_id));
            if scope.fetch_optional(sql).await?.is_some() {
                return Ok(FavoriteAdded::AlreadyThere);
            }
            let mut sql = scope.sql(
                "SELECT COUNT(*), COALESCE(MAX(sort_order), -1) FROM favorites WHERE user_id = ",
            );
            sql.bind(Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            let (count, last) = match scope.fetch_optional(sql).await? {
                Some(row) => (row.int(0)?, row.int(1)?),
                None => (0, -1),
            };
            if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_FAVORITES {
                return Ok(FavoriteAdded::Full);
            }
            let mut sql = scope.sql(
                "INSERT INTO favorites (workspace_id, user_id, target_id, target_kind, \
                 sort_order, created_at) VALUES (",
            );
            sql.changing_no_access()
                .bind(Id(workspace_id))
                .push_bind(", ", Id(user_id))
                .push_bind(", ", Id(&favorite.target_id))
                .push_bind(", ", favorite.kind.as_str())
                .push_bind(", ", last.saturating_add(1))
                .push_bind(", ", now)
                .push(")");
            scope.execute(sql).await?;
            Ok(FavoriteAdded::Added)
        })
        .await
    }

    /// Takes the star of `user_id` off `target_id` in the workspace; whether there was
    /// one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn remove_favorite(
        &self,
        workspace_id: &str,
        user_id: &str,
        target_id: &str,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM favorites WHERE user_id = ");
            sql.changing_no_access()
                .bind(Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id))
                .push_bind(" AND target_id = ", Id(target_id));
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// Puts the stars of `user_id` in the workspace in the order of `target_ids`: those
    /// it names first, as it names them, and the others after them in the order they
    /// had. An identifier of no star of theirs is passed over.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails; nothing moves then.
    pub async fn reorder_favorites(
        &self,
        workspace_id: &str,
        user_id: &str,
        target_ids: &[String],
    ) -> Result<(), StoreError> {
        if target_ids.is_empty() {
            return Ok(());
        }
        let after = i64::try_from(target_ids.len()).unwrap_or(i64::MAX);
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE favorites SET sort_order = CASE target_id");
            sql.changing_no_access();
            for (at, target_id) in (0_i64..).zip(target_ids) {
                sql.push_bind(" WHEN ", Id(target_id))
                    .push_bind(" THEN ", at);
            }
            sql.push_bind(" ELSE sort_order + ", after)
                .push_bind(" END WHERE user_id = ", Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }
}

/// Holds what a write of the stars of `user_id` in the workspace writes under: the
/// workspace, against its deletion, and their membership of it, against another such
/// write meanwhile, in the order of their ranks. Only PostgreSQL has rows to hold: on
/// SQLite a transaction that writes holds the whole database.
async fn hold_stars_of(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM workspaces WHERE id = ");
    sql.bind(Id(workspace_id));
    scope
        .hold(LockRank::Workspace, workspace_id, sql, RowLock::KeyShare)
        .await?;
    let mut sql = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    scope
        .hold(
            LockRank::WorkspaceMembers,
            &format!("{workspace_id}:{user_id}"),
            sql,
            RowLock::NoKeyUpdate,
        )
        .await
}

/// Takes the stars that are on what is gone for good: no block nor container is there
/// of that identifier, in use or in the trash. For the sweep of what blocks leave
/// behind ([`crate::block_derived::forget_gone_blocks`]), within its scope.
pub(crate) async fn forget_gone_targets(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut frag = SqlFragment::of(&["DELETE FROM favorites WHERE"]);
    for (at, (table, alias)) in [
        ("blocks", "b"),
        ("deleted_blocks", "d"),
        ("containers", "c"),
        ("deleted_containers", "dc"),
    ]
    .into_iter()
    .enumerate()
    {
        if at > 0 {
            frag.push(" AND");
        }
        frag.push(" NOT EXISTS (SELECT 1 FROM ")
            .push(table)
            .push(" AS ")
            .push(alias)
            .push(" WHERE ")
            .push(alias)
            .push(".id = favorites.target_id")
            .push_fragment(&scope.same_organization(&[(alias, "favorites")]))
            .push(")");
    }
    let mut sql = scope.sql(&frag);
    sql.changing_no_access();
    scope.execute(sql).await?;
    Ok(())
}

/// Takes the stars of `user_id` in the workspace: they left it.
pub(crate) async fn forget_user_in_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM favorites WHERE user_id = ");
    sql.changing_no_access()
        .bind(Id(user_id))
        .push_bind(" AND workspace_id = ", Id(workspace_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Takes the stars of `user_id` in every workspace of the scope's organization: they
/// left it.
pub(crate) async fn forget_user_in_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM favorites WHERE user_id = ");
    sql.changing_no_access()
        .bind(Id(user_id))
        .push(" AND workspace_id IN (SELECT id FROM workspaces WHERE org_id = ")
        .bind(scope.org()?)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}
