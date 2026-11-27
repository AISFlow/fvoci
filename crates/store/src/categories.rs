//! Sidebar categories: a user's named groups of boards within a workspace.
//!
//! A category is a row while it exists. Deleting one removes the row, and with it the
//! filing of boards under it.

use std::collections::HashSet;

use fvoci_model::{Category, CategoryPatch, CategoryType};

use crate::Store;
use crate::db::{Id, Row, Scope};
use crate::error::StoreError;

/// The distance between the sort orders of consecutive categories. Categories are listed
/// by ascending sort order, so it is also how far a new category pushes the others down.
const CATEGORY_SORT_ORDER_GAP: i64 = 10;

/// The columns of a category, in the order [`category_from_row`] reads them.
const CATEGORY_COLUMNS: &str = "id, name, user_id, workspace_id, created_at, updated_at, \
                                collapsed, sort_order, type";

impl Store {
    /// Returns the category with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such category and
    /// [`StoreError::Corrupt`] if its stored type is not one this server knows.
    pub async fn get_category(&self, id: &str) -> Result<Category, StoreError> {
        self.with_conn(async |scope| get_category(scope, id).await)
            .await
    }

    /// Returns the categories of a user in a workspace, by ascending sort order; those with
    /// equal sort orders come oldest first, then by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] if the stored type of one of them is not one this
    /// server knows, and [`StoreError::Database`] if the query fails.
    pub async fn get_user_categories(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Category>, StoreError> {
        self.with_conn(async |scope| get_user_categories(scope, user_id, workspace_id).await)
            .await
    }

    /// Stores `category` as given and moves every category of its user and workspace down by
    /// one place, so that a category created with sort order zero ends up first.
    ///
    /// The category is not validated and its `sorting` is not stored. The category itself
    /// is among those moved down: it is stored with a sort order ten higher than given.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails, for instance because the
    /// identifier is taken; nothing is written in that case.
    pub async fn create_category(&self, category: &Category) -> Result<(), StoreError> {
        self.transaction(async |scope| create_category(scope, category).await)
            .await
    }

    /// Overwrites the name, the modification time and the collapsed state of the category
    /// that has the identifier of `category`, with the values `category` holds.
    ///
    /// Every other member of `category` is ignored. Updating a category that does not
    /// exist succeeds and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn update_category(&self, category: &Category) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE categories SET ");
            sql.push_bind("name = ", &category.name)
                .push_bind(", updated_at = ", category.updated_at)
                .push_bind(", collapsed = ", category.collapsed)
                .push_bind(" WHERE id = ", Id(&category.id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Deletes a category of a user in a workspace. The boards filed under it are filed under
    /// nothing afterwards.
    ///
    /// Deleting a category that does not exist, or belongs to another user or workspace,
    /// succeeds and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn delete_category(
        &self,
        category_id: &str,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM categories WHERE id = ");
            sql.bind(Id(category_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Partially updates `name` and/or `collapsed` of a user's category in `workspace_id`
    /// in a single transaction, returning the updated category.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the category does not exist or belongs to
    /// another user or workspace, and [`StoreError::InvalidCategory`] if the resulting
    /// category fails validation.
    pub async fn patch_category(
        &self,
        workspace_id: &str,
        category_id: &str,
        user_id: &str,
        patch: &CategoryPatch,
    ) -> Result<Category, StoreError> {
        self.transaction(async |scope| {
            let mut category = get_category(scope, category_id).await?;
            if category.workspace_id != workspace_id || category.user_id != user_id {
                return Err(StoreError::not_found(format_args!(
                    "category ID={category_id}"
                )));
            }
            if category.category_type != CategoryType::System
                && let Some(name) = &patch.name
            {
                category.name = name.trim().to_owned();
            }
            if let Some(collapsed) = patch.collapsed {
                category.collapsed = collapsed;
            }
            category.updated_at = scope.now_millis();
            category.validate()?;
            let mut sql = scope.sql("UPDATE categories SET name = ");
            sql.bind(&category.name)
                .push_bind(", updated_at = ", category.updated_at)
                .push_bind(", collapsed = ", category.collapsed)
                .push_bind(" WHERE id = ", Id(&category.id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found(format_args!(
                    "category ID={category_id}"
                )));
            }
            Ok(category)
        })
        .await
    }

    /// Gives the categories of a user in a workspace the sort orders 0, 10, 20 and so on, in
    /// the order of their identifiers in `new_order`, and returns `new_order`.
    ///
    /// Categories of the user and workspace that `new_order` does not name keep their sort
    /// order, and identifiers that are not categories of the user and workspace are ignored. An empty order changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DuplicateReorderId`] if `new_order` contains duplicate IDs,
    /// and [`StoreError::Database`] if the statement fails.
    pub async fn reorder_categories(
        &self,
        user_id: &str,
        workspace_id: &str,
        new_order: Vec<String>,
    ) -> Result<Vec<String>, StoreError> {
        let mut seen = HashSet::with_capacity(new_order.len());
        for category_id in &new_order {
            if !seen.insert(category_id.as_str()) {
                return Err(StoreError::DuplicateReorderId(category_id.clone()));
            }
        }
        self.with_write_conn(async |scope| {
            if new_order.is_empty() {
                return Ok(());
            }
            let mut sql = scope.sql("UPDATE categories SET sort_order = CASE id");
            // From the first gap, not from 0: a new category is put at 0, before them all.
            let sort_orders = (1..).map(|position: i64| position * CATEGORY_SORT_ORDER_GAP);
            for (sort_order, category_id) in sort_orders.zip(&new_order) {
                sql.push_bind(" WHEN ", Id(category_id))
                    .push_bind(" THEN ", sort_order);
            }
            sql.push(" ELSE sort_order END")
                .push_bind(" WHERE user_id = ", Id(user_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await?;
        Ok(new_order)
    }
}

/// Reads the `type` column. The schema lets it hold the two types only.
fn category_type_from_column(value: &str) -> Result<CategoryType, StoreError> {
    [CategoryType::System, CategoryType::Custom]
        .into_iter()
        .find(|category_type| category_type.as_str() == value)
        .ok_or_else(|| StoreError::Corrupt {
            column: "type",
            reason: format!("unknown category type {value:?}"),
        })
}

fn category_from_row(row: &Row) -> Result<Category, StoreError> {
    Ok(Category {
        id: row.id(0)?,
        name: row.text(1)?,
        user_id: row.id(2)?,
        workspace_id: row.id(3)?,
        created_at: row.int(4)?,
        updated_at: row.int(5)?,
        // A category that exists is not deleted.
        deleted_at: None,
        collapsed: row.bool(6)?,
        sort_order: row.int(7)?,
        category_type: category_type_from_column(&row.text(8)?)?,
    })
}

pub(crate) async fn get_category(scope: &mut Scope<'_>, id: &str) -> Result<Category, StoreError> {
    let mut sql = scope.sql(["SELECT ", CATEGORY_COLUMNS, " FROM categories WHERE id = "]);
    sql.bind(Id(id));
    match scope.fetch_optional(sql).await? {
        Some(row) => category_from_row(&row),
        None => Err(StoreError::not_found(format_args!("category ID={id}"))),
    }
}

pub(crate) async fn get_user_categories(
    scope: &mut Scope<'_>,
    user_id: &str,
    workspace_id: &str,
) -> Result<Vec<Category>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        CATEGORY_COLUMNS,
        " FROM categories WHERE user_id = ",
    ]);
    sql.bind(Id(user_id))
        .push_bind(" AND workspace_id = ", Id(workspace_id))
        .push(" ORDER BY sort_order, created_at, id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(category_from_row)
        .collect()
}

pub(crate) async fn create_category(
    scope: &mut Scope<'_>,
    category: &Category,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "INSERT INTO categories (id, name, user_id, workspace_id, created_at, updated_at, collapsed, \
         sort_order, type) VALUES (",
    );
    sql.bind(Id(&category.id))
        .push_bind(", ", &category.name)
        .push_bind(", ", Id(&category.user_id))
        .push_bind(", ", Id(&category.workspace_id))
        .push_bind(", ", category.created_at)
        .push_bind(", ", category.updated_at)
        .push_bind(", ", category.collapsed)
        .push_bind(", ", category.sort_order)
        .push_bind(", ", category.category_type.as_str())
        .push(")");
    scope.execute(sql).await?;

    // The row inserted above matches this statement too, so the new category moves down
    // with the existing ones and keeps its place relative to them.
    let mut sql = scope.sql("UPDATE categories SET sort_order = sort_order + ");
    sql.bind(CATEGORY_SORT_ORDER_GAP)
        .push_bind(" WHERE user_id = ", Id(&category.user_id))
        .push_bind(" AND workspace_id = ", Id(&category.workspace_id));
    scope.execute(sql).await?;
    Ok(())
}
