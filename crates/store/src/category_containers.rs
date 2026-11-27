//! The boards in each sidebar category.
//!
//! A user has at most one row per board in each organization: the category the board
//! is in, its place among the boards of that category, and whether the user has hidden
//! it. The rows refer to boards by identifier only; nothing here reads the boards
//! themselves.

use std::collections::HashSet;

use fvoci_model::{
    CATEGORY_CONTAINERS_SORT_ORDER_GAP, CategoryContainerMetadata, CategoryContainerUpdate,
    CategoryContainers, CategoryType, SidebarVisibility, new_id,
};

use crate::Store;
use crate::categories::get_user_categories;
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;

impl Store {
    /// Returns the categories of a user in a workspace, in the order of
    /// [`Store::get_user_categories`], each with its boards by ascending sort order.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] if the stored type of a category is not one this
    /// server knows, and [`StoreError::Database`] if a query fails.
    pub async fn get_user_category_containers(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<CategoryContainers>, StoreError> {
        self.with_conn(async |scope| {
            get_user_category_containers(scope, user_id, workspace_id).await
        })
        .await
    }

    /// Puts boards into a category of a user. A board named more than once counts once.
    ///
    /// A board the user has in no category yet is added visible and with sort order zero,
    /// which places it before boards that were reordered. A board the user has in another
    /// category moves to this one and keeps its sort order and its hidden flag. The
    /// category is not checked to belong to the user.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the category, a board or the user does not
    /// exist, and [`StoreError::InvalidId`] for text that is no identifier; nothing is
    /// written in either case.
    pub async fn add_update_category_container(
        &self,
        user_id: &str,
        category_id: &str,
        container_ids: &[String],
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            add_update_category_container(scope, user_id, category_id, container_ids).await
        })
        .await
    }

    /// Gives the boards of a category the sort orders 10, 11, 12 and so on, in the order
    /// of their identifiers in `new_order`, and returns `new_order`.
    ///
    /// Boards of the category that `new_order` does not name keep their sort order, which
    /// may then be that of a board the order names. Identifiers that are not boards of
    /// the category are ignored. An empty order
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn reorder_category_containers(
        &self,
        category_id: &str,
        new_order: Vec<String>,
    ) -> Result<Vec<String>, StoreError> {
        let mut seen = HashSet::with_capacity(new_order.len());
        for container_id in &new_order {
            if !seen.insert(container_id.as_str()) {
                return Err(StoreError::DuplicateReorderId(container_id.clone()));
            }
        }
        self.with_write_conn(async |scope| {
            if new_order.is_empty() {
                return Ok(());
            }
            let mut sql =
                scope.sql("UPDATE category_containers SET sort_order = CASE container_id");
            // Consecutive boards are spaced by 1 starting from the sort-order gap offset.
            for (sort_order, container_id) in (CATEGORY_CONTAINERS_SORT_ORDER_GAP..).zip(&new_order)
            {
                sql.push_bind(" WHEN ", Id(container_id))
                    .push_bind(" THEN ", sort_order);
            }
            // A board of the category that the order leaves out keeps its place.
            sql.push(" ELSE sort_order END")
                .push_bind(" WHERE category_id = ", Id(category_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await?;
        Ok(new_order)
    }

    /// Hides a board in a category of a user, or shows it again.
    ///
    /// Succeeds and changes nothing if the user does not have the board in the category.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn set_container_visibility(
        &self,
        user_id: &str,
        category_id: &str,
        container_id: &str,
        visibility: SidebarVisibility,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE category_containers SET hidden = ");
            sql.bind(visibility == SidebarVisibility::Hidden)
                .push_bind(" WHERE user_id = ", Id(user_id))
                .push_bind(" AND category_id = ", Id(category_id))
                .push_bind(" AND container_id = ", Id(container_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Atomically moves `container_id` into `category_id` (belonging to `user_id` in
    /// `workspace_id`), updates its optional `hidden` flag, and places it at optional
    /// 0-based `index` within `category_id` in a single database transaction.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if `category_id` does not belong to `user_id` in
    /// `workspace_id`, or if `container_id` does not exist.
    pub async fn put_category_container(
        &self,
        user_id: &str,
        workspace_id: &str,
        category_id: &str,
        container_id: &str,
        update: &CategoryContainerUpdate,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            let mut cat_sql = scope.sql("SELECT 1 FROM categories WHERE id = ");
            cat_sql
                .bind(Id(category_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            if scope.fetch_optional(cat_sql).await?.is_none() {
                return Err(StoreError::not_found(format_args!(
                    "category ID={category_id}"
                )));
            }

            let container_ids = [container_id.to_owned()];
            add_update_category_container(scope, user_id, category_id, &container_ids).await?;

            if let Some(hidden) = update.hidden {
                let mut vis = scope.sql("UPDATE category_containers SET hidden = ");
                vis.bind(hidden)
                    .push_bind(", updated_at = ", scope.now_millis())
                    .push_bind(" WHERE user_id = ", Id(user_id))
                    .push_bind(" AND category_id = ", Id(category_id))
                    .push_bind(" AND container_id = ", Id(container_id));
                scope.execute(vis).await?;
            }

            if let Some(target_index) = update.index {
                let mut cur_sql =
                    scope.sql("SELECT container_id FROM category_containers WHERE category_id = ");
                cur_sql
                    .bind(Id(category_id))
                    .push(" ORDER BY sort_order, container_id");
                let mut ordered: Vec<String> = scope
                    .fetch_all(cur_sql)
                    .await?
                    .iter()
                    .map(|row| row.id(0))
                    .collect::<Result<_, _>>()?;
                ordered.retain(|id| id != container_id);
                let pos = target_index.min(ordered.len());
                ordered.insert(pos, container_id.to_owned());

                let mut reorder =
                    scope.sql("UPDATE category_containers SET sort_order = CASE container_id");
                for (sort_order, cid) in (CATEGORY_CONTAINERS_SORT_ORDER_GAP..).zip(&ordered) {
                    reorder
                        .push_bind(" WHEN ", Id(cid))
                        .push_bind(" THEN ", sort_order);
                }
                reorder
                    .push(" ELSE sort_order END")
                    .push_bind(" WHERE category_id = ", Id(category_id));
                scope.execute(reorder).await?;
            }
            Ok(())
        })
        .await
    }
}

/// The key by which a user files a board once: in each organization where rows carry
/// theirs (PostgreSQL), as a built-in template, the system organization's, is filed in
/// each organization the user works in.
fn filed_once(scope: &Scope<'_>) -> &'static str {
    match scope.dialect() {
        Dialect::Sqlite => "(user_id, container_id)",
        Dialect::Postgres => "(user_id, container_id, org_id)",
    }
}

pub(crate) async fn get_user_category_containers(
    scope: &mut Scope<'_>,
    user_id: &str,
    workspace_id: &str,
) -> Result<Vec<CategoryContainers>, StoreError> {
    let categories = get_user_categories(scope, user_id, workspace_id).await?;
    let mut user_category_containers = Vec::with_capacity(categories.len());
    for category in categories {
        let container_metadata = category_container_metadata(scope, &category.id).await?;
        user_category_containers.push(CategoryContainers {
            category,
            container_metadata,
        });
    }
    Ok(user_category_containers)
}

/// Returns the boards of a category by ascending sort order. Boards with equal sort
/// orders, such as those never reordered, come by identifier: in the order they were
/// made in.
async fn category_container_metadata(
    scope: &mut Scope<'_>,
    category_id: &str,
) -> Result<Vec<CategoryContainerMetadata>, StoreError> {
    let mut sql =
        scope.sql("SELECT container_id, hidden FROM category_containers WHERE category_id = ");
    // Filters only by `category_id`; category ownership is enforced on writes in the app layer.
    sql.bind(Id(category_id))
        .push(" ORDER BY sort_order, container_id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(CategoryContainerMetadata {
                container_id: row.id(0)?,
                hidden: row.bool(1)?,
            })
        })
        .collect()
}

pub(crate) async fn add_update_category_container(
    scope: &mut Scope<'_>,
    user_id: &str,
    category_id: &str,
    container_ids: &[String],
) -> Result<(), StoreError> {
    // One statement must not name a board twice: PostgreSQL refuses an upsert that would
    // touch the same row a second time. Deduplicate while preserving the first-seen order.
    let mut seen = HashSet::with_capacity(container_ids.len());
    let container_ids: Vec<&str> = container_ids
        .iter()
        .map(String::as_str)
        .filter(|&container_id| seen.insert(container_id))
        .collect();
    if container_ids.is_empty() {
        return Ok(());
    }

    let now = scope.now_millis();
    let mut sql = scope.sql("INSERT INTO category_containers (id, user_id, category_id, container_id, created_at, updated_at, sort_order, \
         hidden) VALUES ");
    for (index, container_id) in container_ids.into_iter().enumerate() {
        sql.push(if index == 0 { "(" } else { ", (" })
            .bind(Id(&new_id()))
            .push_bind(", ", Id(user_id))
            .push_bind(", ", Id(category_id))
            .push_bind(", ", Id(container_id))
            .push_bind(", ", now)
            .push_bind(", ", now)
            .push(", 0, false)");
    }
    // Moving a board to another category keeps its previous sort order and hidden flag.
    sql.push(" ON CONFLICT ").push(filed_once(scope)).push(
        " DO UPDATE SET category_id = EXCLUDED.category_id, updated_at = EXCLUDED.updated_at",
    );
    scope
        .execute(sql)
        .await
        .map_err(|error| error.missing_reference("category, board or user"))?;
    Ok(())
}

/// Where a user filed a board: the category, the board's place in it, and whether it is
/// hidden there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPlace {
    /// The user who filed the board.
    pub user_id: String,
    /// The category the board is filed under.
    pub category_id: String,
    /// The board's sort order in the category.
    pub sort_order: i64,
    /// Whether the board is hidden in the category.
    pub hidden: bool,
}

impl Store {
    /// Returns where users filed a board, by user.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_container_places(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerPlace>, StoreError> {
        self.with_conn(async |scope| places_of_container(scope, container_id).await)
            .await
    }
}

pub(crate) async fn places_of_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Vec<ContainerPlace>, StoreError> {
    let mut sql = scope.sql(
        "SELECT user_id, category_id, sort_order, hidden FROM category_containers WHERE container_id = ",
    );
    sql.bind(Id(container_id)).push(" ORDER BY user_id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(ContainerPlace {
                user_id: row.id(0)?,
                category_id: row.id(1)?,
                sort_order: row.int(2)?,
                hidden: row.bool(3)?,
            })
        })
        .collect()
}

/// Files a restored board where `place` says. Under its category if the user still has
/// it in the board's workspace, at its sort order, the boards from there on moving down by one
/// so that it is between the ones it was between. Else under the user's default category
/// (found by its name, as the app finds it) with sort order 0, as a newly filed board. A
/// user without a default category yet is filed nowhere: making that category files the
/// boards the user is a member of. A category of another workspace, where a board could be
/// filed by naming it before the routes compared the workspaces, counts as gone.
pub(crate) async fn put_back(
    scope: &mut Scope<'_>,
    container_id: &str,
    workspace_id: &str,
    place: &ContainerPlace,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id FROM categories WHERE id = ");
    sql.bind(Id(&place.category_id))
        .push_bind(" AND user_id = ", Id(&place.user_id))
        .push_bind(" AND workspace_id = ", Id(workspace_id));
    let (category_id, sort_order) = if let Some(row) = scope.fetch_optional(sql).await? {
        let category_id = row.id(0)?;
        let mut sql = scope
            .sql("UPDATE category_containers SET sort_order = sort_order + 1 WHERE category_id = ");
        sql.bind(Id(&category_id))
            .push_bind(" AND sort_order >= ", place.sort_order);
        scope.execute(sql).await?;
        (category_id, place.sort_order)
    } else {
        let mut sql = scope.sql("SELECT id FROM categories WHERE type = ");
        sql.bind(CategoryType::System.as_str())
            .push_bind(" AND user_id = ", Id(&place.user_id))
            .push_bind(" AND workspace_id = ", Id(workspace_id))
            .push(" ORDER BY sort_order, created_at, id LIMIT 1");
        match scope.fetch_optional(sql).await? {
            Some(row) => (row.id(0)?, 0),
            None => return Ok(()),
        }
    };
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO category_containers \
         (id, user_id, category_id, container_id, created_at, updated_at, sort_order, hidden) VALUES (",
    );
    sql.bind(Id(&new_id()))
        .push_bind(", ", Id(&place.user_id))
        .push_bind(", ", Id(&category_id))
        .push_bind(", ", Id(container_id))
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push_bind(", ", sort_order)
        .push_bind(", ", place.hidden)
        .push(") ON CONFLICT ")
        .push(filed_once(scope))
        .push(" DO NOTHING");
    scope.execute(sql).await?;
    Ok(())
}
