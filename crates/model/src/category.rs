//! Sidebar categories, which group a user's boards within a workspace.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::id::new_id;
use crate::refusal::{RefusalReason, Refusals};

/// The spacing between the sort orders of consecutive boards in a category.
pub const CATEGORY_CONTAINERS_SORT_ORDER_GAP: i64 = 10;

/// The name of the category every user has, which collects the boards filed nowhere
/// else; it is found by this name.
pub const DEFAULT_CATEGORY_NAME: &str = "Boards";

/// Whether a category was created by the server or by the user.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum CategoryType {
    /// A built-in category, such as the default "Boards" one.
    System,
    /// A category the user created.
    #[default]
    Custom,
}

impl CategoryType {
    /// Returns the wire and database form.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Custom => "custom",
        }
    }
}

/// Whether a board shows in its user's sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidebarVisibility {
    /// The board shows.
    Visible,
    /// The user has hidden the board.
    Hidden,
}

/// Why a [`Category`] is rejected before it is stored: every member that is refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct CategoryError(Refusals);

impl CategoryError {
    /// The members that were refused.
    pub fn into_refusals(self) -> Refusals {
        self.0
    }
}

/// A sidebar category of one user in one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Category {
    /// The category's identifier.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The identifier of the user the category belongs to.
    pub user_id: String,
    /// The identifier of the workspace the category belongs to.
    pub workspace_id: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Deletion time in milliseconds since the Unix epoch; absent while not deleted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
    /// Whether the category is collapsed in the sidebar.
    pub collapsed: bool,
    /// The position of the category among the user's categories.
    pub sort_order: i64,
    /// Whether this is a system or a custom category.
    #[serde(rename = "type")]
    pub category_type: CategoryType,
}

impl Category {
    /// Fills in the members a new category may leave out: a fresh identifier, `now`
    /// (milliseconds since the epoch) as creation time, the creation time as modification
    /// time, and zero for a negative sort order.
    pub fn hydrate(&mut self, now: i64) {
        if self.id.is_empty() {
            self.id = new_id();
        }
        if self.created_at == 0 {
            self.created_at = now;
        }
        if self.updated_at == 0 {
            self.updated_at = self.created_at;
        }
        if self.sort_order < 0 {
            self.sort_order = 0;
        }
    }

    /// Checks that the category may be stored.
    ///
    /// # Errors
    ///
    /// Returns a [`CategoryError`] with every one of the identifier, name, user and workspace
    /// that is blank.
    pub fn validate(&self) -> Result<(), CategoryError> {
        let required = [
            (&self.id, "id", "the category has no id"),
            (&self.name, "name", "the category has no name"),
            (&self.user_id, "userId", "the category names no user"),
            (
                &self.workspace_id,
                "workspaceId",
                "the category names no workspace",
            ),
        ];
        let mut refused = Refusals::new();
        for (value, member, message) in required {
            if value.trim().is_empty() {
                refused.refuse(member, RefusalReason::Required, message);
            }
        }
        refused.into_result().map_err(CategoryError)
    }
}

/// A board's place in a category.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoryContainerMetadata {
    /// The identifier of the board.
    pub container_id: String,
    /// Whether the user has hidden the board from the sidebar.
    pub hidden: bool,
}

/// A category together with the boards in it, in display order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct CategoryContainers {
    /// The category. Its members appear inline in the JSON form.
    #[serde(flatten)]
    pub category: Category,
    /// The boards in the category.
    pub container_metadata: Vec<CategoryContainerMetadata>,
}

/// The payload of the websocket message sent when a board or a wiki moves between categories
/// or is hidden.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContainerCategoryWebsocketData {
    /// The identifier of the board or wiki.
    pub container_id: String,
    /// The identifier of the category it is now in.
    pub category_id: String,
    /// Whether it is hidden from the sidebar.
    pub hidden: bool,
}
