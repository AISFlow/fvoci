//! The boards that exist, board membership and board templates.
//!
//! Deleting a board moves it to the trash and restoring brings it back; both are in
//! [`crate::trash`]. Memberships are kept in a table of their own and go with their
//! board.

use std::collections::BTreeSet;

use fvoci_model::{
    CardPropertyDefinition, Container, ContainerError, ContainerKind, ContainerKinds,
    ContainerMember, ContainerPatch, ContainerRole, ContainerVisibility, DoneWhen, MinimumRole,
    PublicContainers,
};
use serde_json::Value;

use crate::Store;
// Re-exported through `containers` so newer modules avoid naming the legacy table module.
pub(crate) use crate::container_team_access::inherited_members_source;
use crate::db::{FromRow, Id, LockRank, OptId, Param, Row, RowLock, Scope, Sql};
use crate::error::StoreError;
use crate::{block_derived, container_kinds, members, subscriptions};

/// The order of a list of boards read through the alias `b`: by when they were made,
/// and by identifier where that is the same.
pub(crate) const OLDEST_CONTAINER_FIRST: &str = " ORDER BY b.created_at, b.id";

pub use crate::container_team_access::ContainerAccessFacts;

/// The name of every board column, in the order rows are read and written.
pub(crate) const CONTAINER_COLUMNS: &str = "id, workspace_id, created_by, modified_by, \
     visibility, minimum_role, title, description, icon, show_description, is_template, \
     is_builtin, template_version, properties, card_properties, created_at, updated_at, kind, \
     due_date_property_id, done_when";

/// Select list of every board column qualified with alias `b.`.
pub(crate) const CONTAINER_SELECT_B: &str = "b.id, b.workspace_id, b.created_by, b.modified_by, \
     b.visibility, b.minimum_role, b.title, b.description, b.icon, b.show_description, \
     b.is_template, b.is_builtin, b.template_version, b.properties, b.card_properties, \
     b.created_at, b.updated_at, b.kind, b.due_date_property_id, b.done_when";

/// Number of columns in [`CONTAINER_COLUMNS`] and [`CONTAINER_SELECT_B`].
pub(crate) const CONTAINER_COLUMN_COUNT: usize = 20;

/// What joining a board did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Joined {
    /// The user was no member and is one now, with the board's minimum role.
    New(ContainerMember),
    /// The user was a member below the board's minimum role and has that role now.
    Raised(ContainerMember),
    /// The user was a member with the minimum role or a higher one; nothing changed.
    Already(ContainerMember),
}

impl Joined {
    /// The membership as it is after joining.
    #[cfg(any(test, feature = "test-util"))]
    pub fn into_member(self) -> ContainerMember {
        match self {
            Self::New(member) | Self::Raised(member) | Self::Already(member) => member,
        }
    }
}

/// What became of a change to a membership that must leave its board an administrator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberChange {
    /// The change was made.
    Done,
    /// The user is no member of the board; nothing changed.
    NotAMember,
    /// The user is the board's only administrator and the change would have ended
    /// that; nothing changed.
    LastAdmin,
}

impl Store {
    /// Inserts `container`, or updates the board that already has its identifier.
    ///
    /// `container` is updated to what was stored: `user_id` as modifier and the current time
    /// as modification time and, for a new board, `user_id` as creator and the current
    /// time as creation time. An update leaves the stored workspace, creator and creation time
    /// alone, and `container` comes back with those.
    ///
    /// A template stored on behalf of no user (the empty `user_id`) is a built-in
    /// template: `is_builtin` is set, whatever `container` says. No other board is
    /// built-in, and an update never changes whether a board is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidContainer`], and nothing written, for a board that may
    /// not be stored ([`Container::check_storable`]) and for one that exists as of another
    /// kind; [`StoreError::Database`] if a statement fails.
    pub async fn insert_container(
        &self,
        container: &mut Container,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| insert_container(scope, container, user_id).await)
            .await
    }

    /// Inserts or updates `container` as [`Store::insert_container`] does and makes its creator
    /// an administrator and editor of it. Returns that membership.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if its creator does not work there, or a database error.
    pub async fn insert_container_with_admin(
        &self,
        container: &mut Container,
        user_id: &str,
    ) -> Result<ContainerMember, StoreError> {
        self.transaction(async |scope| insert_container_with_admin(scope, container, user_id).await)
            .await
    }

    /// Applies `patch` to the board with the given identifier and returns the patched
    /// board.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board.
    pub async fn patch_container(
        &self,
        container_id: &str,
        patch: &ContainerPatch,
        user_id: &str,
    ) -> Result<Container, StoreError> {
        self.transaction(async |scope| patch_container(scope, container_id, patch, user_id).await)
            .await
    }

    /// Puts the card properties of a board into the order of their identifiers in
    /// `order`, which names each of them once, and returns the board, marked as modified
    /// by `user_id` now. The board is held from before it is read.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board, and
    /// [`StoreError::InvalidContainer`], and nothing written, if `order` does not name each
    /// property once ([`Container::reorder_card_properties`]).
    pub async fn reorder_card_properties(
        &self,
        container_id: &str,
        order: &[String],
        user_id: &str,
    ) -> Result<Container, StoreError> {
        self.transaction(async |scope| {
            hold_container(scope, container_id).await?;
            let mut container = get_container(scope, container_id).await?;
            container.reorder_card_properties(order)?;
            insert_container(scope, &mut container, user_id).await?;
            Ok(container)
        })
        .await
    }

    /// Returns the board with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board, and
    /// [`StoreError::Corrupt`] if its stored type or minimum role is not a known one.
    pub async fn get_container(&self, container_id: &str) -> Result<Container, StoreError> {
        self.with_conn(async |scope| get_container(scope, container_id).await)
            .await
    }

    /// Returns the boards of `kinds` of a workspace that a user is a member of and, with
    /// [`PublicContainers::Include`], the open ones of the workspace (with
    /// [`PublicContainers::OpenToRead`], its open wikis and courses), oldest first.
    /// Templates are left out.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_containers_for_user_and_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
        public_containers: PublicContainers,
        kinds: ContainerKinds,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_containers_of_user(scope, user_id, public_containers, kinds);
            sql.push_bind(" AND b.workspace_id = ", Id(workspace_id))
                .push(OLDEST_CONTAINER_FIRST);
            fetch_containers(scope, sql).await
        })
        .await
    }

    /// Returns the boards of a workspace with the given identifiers, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotAllFound`] unless exactly one board is found per identifier.
    pub async fn get_containers_in_workspace_by_ids(
        &self,
        container_ids: &[String],
        workspace_id: &str,
    ) -> Result<Vec<Container>, StoreError> {
        let ids = container_ids;
        self.with_conn(async |scope| {
            get_containers_in_workspace_by_ids(scope, ids, workspace_id).await
        })
        .await
    }

    /// Makes a user a member of a board with the `role` of `member`, or replaces the
    /// `role` of an existing membership, whatever that does to the board: for tests that
    /// need a membership the checked ways would not make. The server uses
    /// [`Store::add_member`], [`Store::join_container`] and [`Store::change_member`].
    ///
    /// The `synthetic` and `inherited_role` members of `member` are not stored.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    #[cfg(feature = "test-util")]
    pub async fn save_member(&self, member: &ContainerMember) -> Result<(), StoreError> {
        self.transaction(async |scope| save_member(scope, member).await)
            .await
    }

    /// Removes a user from a board and ends the user's subscriptions to the blocks of
    /// the board, also if that leaves the board without an administrator: for tests.
    /// The server uses [`Store::remove_member`]. Removing a user who is not a member
    /// succeeds and changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    #[cfg(feature = "test-util")]
    pub async fn delete_member(&self, container_id: &str, user_id: &str) -> Result<(), StoreError> {
        self.transaction(async |scope| delete_member(scope, container_id, user_id).await)
            .await
    }

    /// Makes a user a member of a board with the board's minimum role, as joining does.
    ///
    /// A user who is no member becomes one with that role. A member whose role is below
    /// it is raised to it. A member with that role or a higher one stays as they are:
    /// joining never lowers anybody.
    ///
    /// Reading the board and the membership and writing the membership are one step.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board or no such user.
    pub async fn join_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<Joined, StoreError> {
        self.transaction(async |scope| {
            // Nobody joins a board the handle reads and may not change (a built-in template).
            hold_container(scope, container_id).await?;
            let container = get_container(scope, container_id).await?;
            let minimum = ContainerRole::from(container.minimum_role);
            let with_minimum = ContainerMember::new(container_id, user_id, minimum);
            match find_member(scope, container_id, user_id).await? {
                None => {
                    save_member(scope, &with_minimum)
                        .await
                        .map_err(|error| error.missing_reference("user"))?;
                    Ok(Joined::New(with_minimum))
                }
                Some(member) if member.role < minimum => {
                    save_member(scope, &with_minimum).await?;
                    Ok(Joined::Raised(with_minimum))
                }
                Some(member) => Ok(Joined::Already(member)),
            }
        })
        .await
    }

    /// Removes a user from a board and ends the user's subscriptions to the blocks of
    /// the board, unless that would leave the board without an administrator.
    ///
    /// The check and the removal are one step: of two administrators who leave at once,
    /// one is refused.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn remove_member(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<MemberChange, StoreError> {
        self.transaction(async |scope| {
            if !lock_container(scope, container_id).await? {
                return Ok(MemberChange::NotAMember);
            }
            let Some(current) = find_member(scope, container_id, user_id).await? else {
                return Ok(MemberChange::NotAMember);
            };
            if current.role == ContainerRole::Admin
                && members::is_only_admin(scope, container_id, user_id).await?
            {
                return Ok(MemberChange::LastAdmin);
            }
            delete_member(scope, container_id, user_id).await?;
            Ok(MemberChange::Done)
        })
        .await
    }

    /// Returns a user's membership of a board.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the user is not a member of the board.
    pub async fn get_member_for_container(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<ContainerMember, StoreError> {
        self.with_conn(async |scope| get_member_for_container(scope, container_id, user_id).await)
            .await
    }

    /// Returns the members of a board, by the identifier of the user: the users in the
    /// order their accounts were made in.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_members_for_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.with_conn(async |scope| get_members_for_container(scope, container_id).await)
            .await
    }

    /// Returns a user's memberships, by the identifier of the board: the boards in the
    /// order they were made in. A membership of a board of a workspace the user is no
    /// member of any more is left out.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_members_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.with_conn(async |scope| {
            crate::container_team_access::get_members_for_user(scope, user_id).await
        })
        .await
    }

    /// Returns the templates of a workspace that are open or that the user is a member of,
    /// oldest first. The built-in templates are not among them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_template_boards(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| {
            crate::workspaces::get_template_boards(scope, workspace_id, user_id).await
        })
        .await
    }

    /// Returns the templates the server ships with, oldest first. They are the system
    /// organization's, in its workspace: every organization is offered them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_builtin_templates(&self) -> Result<Vec<Container>, StoreError> {
        self.with_conn(get_builtin_templates).await
    }

    /// Removes the built-in templates together with their blocks, for good: they do not
    /// go to the trash. For tests: it neither takes the lock of
    /// [`Store::replace_builtin_templates`] nor hands back the templates' files, so the
    /// server replaces its templates with that.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is removed then.
    pub async fn remove_builtin_templates(&self) -> Result<(), StoreError> {
        self.transaction(remove_builtin_templates).await
    }
}

/// Starts `SELECT <board columns> FROM containers`.
fn select_containers(scope: &Scope<'_>) -> Sql {
    scope.sql(["SELECT ", CONTAINER_COLUMNS, " FROM containers"])
}

pub(crate) async fn fetch_containers(
    scope: &mut Scope<'_>,
    sql: Sql,
) -> Result<Vec<Container>, StoreError> {
    scope.all(sql).await
}

impl FromRow for Container {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        container_from_row_at(row, 0)
    }
}

pub(crate) fn container_from_row_at(row: &Row, base: usize) -> Result<Container, StoreError> {
    Ok(Container {
        id: row.id(base)?,
        workspace_id: row.id(base + 1)?,
        created_by: row.id(base + 2)?,
        modified_by: row.id(base + 3)?,
        visibility: visibility_from_text(&row.text(base + 4)?)?,
        kind: container_kinds::kind_from_text(&row.text(base + 17)?)?,
        minimum_role: minimum_role_from_text(&row.text(base + 5)?)?,
        title: row.text(base + 6)?,
        description: row.text(base + 7)?,
        icon: row.text(base + 8)?,
        show_description: row.bool(base + 9)?,
        is_template: row.bool(base + 10)?,
        is_builtin: row.bool(base + 11)?,
        template_version: row.int(base + 12)?,
        properties: row.json_map(base + 13, "properties")?,
        card_properties: card_properties_from_json(row.json(base + 14, "card_properties")?)?,
        due_date_property_id: row.opt_text(base + 18)?.filter(|id| !id.is_empty()),
        done_when: done_when_from_json(row.json(base + 19, "done_when")?)?,
        created_at: row.int(base + 15)?,
        updated_at: row.int(base + 16)?,
        // A board that exists is not deleted.
        deleted_at: None,
    })
}

fn done_when_from_json(value: Value) -> Result<Option<DoneWhen>, StoreError> {
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| StoreError::Corrupt {
            column: "done_when",
            reason: error.to_string(),
        })
}

fn visibility_from_text(text: &str) -> Result<ContainerVisibility, StoreError> {
    text.parse().map_err(|_| StoreError::Corrupt {
        column: "visibility",
        reason: format!("unknown board visibility {text:?}"),
    })
}

fn minimum_role_from_text(text: &str) -> Result<MinimumRole, StoreError> {
    MinimumRole::ALL
        .into_iter()
        .find(|role| role.as_str() == text)
        .ok_or_else(|| StoreError::Corrupt {
            column: "minimum_role",
            reason: format!("unknown minimum role {text:?}"),
        })
}

/// Reads the card property definitions of a board. JSON `null` reads as no definitions.
fn card_properties_from_json(value: Value) -> Result<Vec<CardPropertyDefinition>, StoreError> {
    let definitions: Option<Vec<CardPropertyDefinition>> =
        serde_json::from_value(value).map_err(|error| StoreError::Corrupt {
            column: "card_properties",
            reason: error.to_string(),
        })?;
    Ok(definitions.unwrap_or_default())
}

pub(crate) async fn find_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Option<Container>, StoreError> {
    let mut sql = select_containers(scope);
    sql.push_bind(" WHERE id = ", Id(container_id));
    Ok(fetch_containers(scope, sql).await?.into_iter().next())
}

pub(crate) async fn get_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Container, StoreError> {
    find_container(scope, container_id)
        .await?
        .ok_or_else(|| StoreError::not_found("boards"))
}

/// Starts the query for the boards of `kinds` a user can reach: `SELECT DISTINCT` over
/// boards joined with their memberships, without templates, narrowed to the user's boards
/// and, if asked for, the open ones where the user works. A membership counts while the
/// user is a member of the board's workspace, and if it has a role. More `AND` conditions
/// on `b` may follow.
pub(crate) fn select_containers_of_user(
    scope: &Scope<'_>,
    user_id: &str,
    public_containers: PublicContainers,
    kinds: ContainerKinds,
) -> Sql {
    let mut sql = scope.sql([
        "SELECT DISTINCT ",
        CONTAINER_SELECT_B,
        " FROM containers AS b LEFT JOIN container_members AS bm ON b.id = bm.container_id \
         WHERE b.is_template = ",
    ]);
    sql.bind(false);
    container_kinds::push_kinds(&mut sql, "b.kind", kinds);
    match public_containers {
        PublicContainers::Include => {
            members::push_open_containers_at_work(&mut sql, user_id, ContainerKinds::EVERY)
        }
        PublicContainers::OpenToRead => {
            members::push_open_containers_at_work(&mut sql, user_id, ContainerKinds::OPEN_TO_READ)
        }
        PublicContainers::Exclude => sql.push(" AND ("),
    };
    sql.push("(");
    members::push_own_membership_in_workspace(&mut sql, user_id).push(") OR ");
    crate::container_team_access::push_has_team_role(&mut sql, scope, user_id);
    sql.push(")");
    sql
}

pub(crate) async fn get_containers_in_workspace_by_ids(
    scope: &mut Scope<'_>,
    container_ids: &[String],
    workspace_id: &str,
) -> Result<Vec<Container>, StoreError> {
    let mut sql = select_containers(scope);
    sql.push_bind(" WHERE workspace_id = ", Id(workspace_id))
        .push(" AND id IN (")
        .bind_list(container_ids.iter().map(|id| Id(id)))
        .push(") ORDER BY created_at, id");
    let containers = fetch_containers(scope, sql).await?;
    if containers.len() == container_ids.len() {
        Ok(containers)
    } else {
        tracing::warn!(
            found = containers.len(),
            asked = container_ids.len(),
            "get_containers_in_workspace_by_ids: mismatched number of boards found"
        );
        Err(StoreError::NotAllFound {
            entity: "board",
            ids: container_ids.to_vec(),
        })
    }
}

pub(crate) async fn insert_container(
    scope: &mut Scope<'_>,
    container: &mut Container,
    user_id: &str,
) -> Result<(), StoreError> {
    // Every way a board is stored comes through here: created, patched, duplicated,
    // imported.
    container.check_storable()?;
    // A board that exists is held while it is read and written.
    lock_container(scope, &container.id).await?;
    let existing = find_container(scope, &container.id).await?;
    // A template stored on behalf of no user is the system's own: a built-in template.
    // Nothing a caller puts into `container` changes that, and it never changes afterwards.
    container.is_builtin = match &existing {
        Some(existing) => existing.is_builtin,
        None => user_id.is_empty() && container.is_template,
    };
    let now = scope.now_millis();
    user_id.clone_into(&mut container.modified_by);
    container.updated_at = now;
    container.deleted_at = None;

    if let Some(existing) = existing {
        // A board is of the kind it was made as: nothing stores it as of another.
        if existing.kind != container.kind {
            return Err(ContainerError::KindChanges {
                stored: existing.kind,
                asked: container.kind,
            }
            .into());
        }
        let derived_changed = container.kind == ContainerKind::Board
            && (existing.card_properties != container.card_properties
                || existing.due_date_property_id != container.due_date_property_id
                || existing.done_when != container.done_when
                || existing.is_template != container.is_template);
        let access_unchanged = existing.visibility == container.visibility
            && existing.minimum_role == container.minimum_role
            && existing.is_template == container.is_template
            && existing.card_properties == container.card_properties;
        // A board stays in its workspace, and who made it and when does not change: `container`
        // comes back with what is stored, whatever the caller put there.
        container.workspace_id = existing.workspace_id;
        container.created_by = existing.created_by;
        container.created_at = existing.created_at;
        let mut sql = scope.sql("UPDATE containers SET ");
        if access_unchanged {
            sql.changing_no_access();
        }
        sql.push_bind("modified_by = ", OptId(&container.modified_by))
            .push_bind(", visibility = ", container.visibility.as_str())
            .push_bind(", minimum_role = ", container.minimum_role.as_str())
            .push_bind(", title = ", &container.title)
            .push_bind(", description = ", &container.description)
            .push_bind(", icon = ", &container.icon)
            .push_bind(", show_description = ", container.show_description)
            .push_bind(", is_template = ", container.is_template)
            .push_bind(", template_version = ", container.template_version)
            .push_bind(", properties = ", Param::jsonb(&container.properties)?)
            .push_bind(
                ", card_properties = ",
                Param::jsonb(&container.card_properties)?,
            )
            .push_bind(
                ", due_date_property_id = ",
                Param::OptText(container.due_date_property_id.clone()),
            )
            .push_bind(
                ", done_when = ",
                Param::opt_jsonb(container.done_when.as_ref())?,
            )
            .push_bind(", updated_at = ", container.updated_at)
            .push_bind(" WHERE id = ", Id(&container.id));
        if scope.execute(sql).await? == 0 {
            return Err(StoreError::not_found(format_args!(
                "board ID={}",
                container.id
            )));
        }
        if derived_changed {
            block_derived::refresh_board_cards(scope, container).await?;
        }
        Ok(())
    } else {
        user_id.clone_into(&mut container.created_by);
        container.created_at = now;
        insert_container_row(scope, container).await
    }
}

/// Inserts a board row with every column as `container` has it. For a new board, and for
/// one that comes back from the trash. A diary is put down only where one may be
/// ([`crate::diaries::check_place`]).
pub(crate) async fn insert_container_row(
    scope: &mut Scope<'_>,
    container: &Container,
) -> Result<(), StoreError> {
    crate::diaries::check_place(scope, container).await?;
    let mut sql = scope.sql(["INSERT INTO containers (", CONTAINER_COLUMNS, ") VALUES ("]);
    sql.bind(Id(&container.id))
        .push_bind(", ", Id(&container.workspace_id))
        .push_bind(", ", OptId(&container.created_by))
        .push_bind(", ", OptId(&container.modified_by))
        .push_bind(", ", container.visibility.as_str())
        .push_bind(", ", container.minimum_role.as_str())
        .push_bind(", ", &container.title)
        .push_bind(", ", &container.description)
        .push_bind(", ", &container.icon)
        .push_bind(", ", container.show_description)
        .push_bind(", ", container.is_template)
        .push_bind(", ", container.is_builtin)
        .push_bind(", ", container.template_version)
        .push_bind(", ", Param::jsonb(&container.properties)?)
        .push_bind(", ", Param::jsonb(&container.card_properties)?)
        .push_bind(", ", container.created_at)
        .push_bind(", ", container.updated_at)
        .push_bind(", ", container.kind.as_str())
        .push_bind(", ", Param::OptText(container.due_date_property_id.clone()))
        .push_bind(", ", Param::opt_jsonb(container.done_when.as_ref())?)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

pub(crate) async fn insert_container_with_admin(
    scope: &mut Scope<'_>,
    container: &mut Container,
    user_id: &str,
) -> Result<ContainerMember, StoreError> {
    insert_container(scope, container, user_id).await?;
    // Its admin is its creator (`user_id`, or the stored one of a board that existed).
    members::hold_creator(scope, &container.workspace_id, &container.created_by).await?;
    let member = ContainerMember::new(&container.id, &container.created_by, ContainerRole::Admin);
    save_member(scope, &member).await?;
    Ok(member)
}

pub(crate) async fn patch_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    patch: &ContainerPatch,
    user_id: &str,
) -> Result<Container, StoreError> {
    // Before the board is looked at: applying the patch would drop a definition without
    // an identifier silently.
    patch.validate().map_err(ContainerError::from)?;
    // Held before the board is read: of two patches at once, the second is applied to
    // what the first left.
    hold_container(scope, container_id).await?;
    let mut container = get_container(scope, container_id).await?;
    patch.apply(&mut container);
    insert_container(scope, &mut container, user_id).await?;
    Ok(container)
}

pub(crate) fn role_from_text(text: &str) -> Result<ContainerRole, StoreError> {
    text.parse().map_err(|_| StoreError::Corrupt {
        column: "role",
        reason: format!("unknown container member role {text:?}"),
    })
}

/// Starts the query for memberships.
pub(crate) fn select_members(scope: &Scope<'_>) -> Sql {
    scope.sql("SELECT bm.container_id, bm.user_id, bm.role FROM container_members AS bm")
}

pub(crate) async fn fetch_members(
    scope: &mut Scope<'_>,
    sql: Sql,
) -> Result<Vec<ContainerMember>, StoreError> {
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(ContainerMember {
                container_id: row.id(0)?,
                user_id: row.id(1)?,
                role: role_from_text(&row.text(2)?)?,
                synthetic: false,
                inherited_role: None,
            })
        })
        .collect()
}

pub(crate) async fn find_member(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<Option<ContainerMember>, StoreError> {
    let mut sql = select_members(scope);
    sql.push_bind(" WHERE bm.container_id = ", Id(container_id))
        .push_bind(" AND bm.user_id = ", Id(user_id));
    Ok(fetch_members(scope, sql).await?.into_iter().next())
}

pub(crate) async fn get_member_for_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<ContainerMember, StoreError> {
    find_member(scope, container_id, user_id)
        .await?
        .ok_or_else(|| {
            StoreError::not_found(format_args!(
                "board member BoardID={container_id} UserID={user_id}"
            ))
        })
}

pub(crate) async fn get_members_for_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Vec<ContainerMember>, StoreError> {
    let mut sql = select_members(scope);
    sql.push_bind(" WHERE bm.container_id = ", Id(container_id))
        .push(" ORDER BY bm.user_id");
    fetch_members(scope, sql).await
}

pub(crate) async fn save_member(
    scope: &mut Scope<'_>,
    member: &ContainerMember,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("INSERT INTO container_members (container_id, user_id, role) VALUES (");
    sql.bind(Id(&member.container_id))
        .push_bind(", ", Id(&member.user_id))
        .push_bind(", ", member.role.as_str())
        .push(") ON CONFLICT (container_id, user_id) DO UPDATE SET role = EXCLUDED.role");
    scope.execute(sql).await?;
    Ok(())
}

/// Makes the transaction the only one that changes what is on the board, and who is
/// on it, until it ends; says whether the board exists.
///
/// What a write to a board decides on, it reads first: that a block's parent is there,
/// what is below a block that is deleted, whether there is another administrator. This
/// keeps what was read true until the write that relied on it. Every such write takes
/// the lock before it reads, so they queue up per board and read what the one before
/// them left.
///
/// On SQLite a transaction that writes holds the whole database from its start. On
/// PostgreSQL the board's row is locked, in the mode that leaves alone rows of other
/// tables that are stored for the board at the same time.
pub(crate) async fn lock_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<bool, StoreError> {
    Ok(lock_container_of_kind(scope, container_id).await?.is_some())
}

/// Locks a board as [`lock_container`] does, and says of what kind it is, which stays as
/// it is whatever is written; `None` if there is no such board.
pub(crate) async fn lock_container_of_kind(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Option<ContainerKind>, StoreError> {
    let mut sql = scope.sql("SELECT id, kind FROM containers WHERE id = ");
    sql.bind(Id(container_id));
    scope
        .first_held::<(String, String)>(LockRank::Container, sql, RowLock::NoKeyUpdate)
        .await?
        .map(|(_, kind)| container_kinds::kind_from_text(&kind))
        .transpose()
}

/// Locks a board as [`lock_container`] does; one that is not there is not found.
pub(crate) async fn hold_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<(), StoreError> {
    if lock_container(scope, container_id).await? {
        return Ok(());
    }
    Err(StoreError::not_found(format_args!(
        "board ID={container_id}"
    )))
}

/// Holds a board that is about to be deleted against everything that is stored for it:
/// the lock the deletion of the row takes anyway, taken before the board is read.
///
/// [`lock_container`] leaves alone whoever stores a row that refers to the board (a
/// membership, a file record, a filing). A row stored between the reading of what goes
/// to the trash and the deletion would go with the board and not be in the trash. This
/// lock waits for such rows that are being stored and shuts out later ones, which then
/// find no board.
pub(crate) async fn lock_container_for_deletion(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT id FROM containers WHERE id = ");
    sql.bind(Id(container_id));
    let held: Option<String> = scope
        .first_held(LockRank::Container, sql, RowLock::Update)
        .await?;
    Ok(held.is_some())
}

/// Takes the lock of [`lock_container_for_deletion`] on the boards `container_ids`, in
/// the order of their identifiers, as [`lock_containers`] does. For a write that
/// deletes more than one board, before it holds anything under them: a board's lock
/// made stronger once its blocks are held, or a later board, waits for those who hold
/// it more weakly while they may wait for what this write holds.
pub(crate) async fn lock_containers_for_deletion(
    scope: &mut Scope<'_>,
    container_ids: &BTreeSet<&str>,
) -> Result<(), StoreError> {
    for container_id in container_ids {
        lock_container_for_deletion(scope, container_id).await?;
    }
    Ok(())
}

/// Takes the lock of [`lock_container`] on several boards, in the order of their
/// identifiers whatever order they are given in: two writes that span the same boards
/// then queue up instead of each holding what the other waits for. For a write that
/// touches more than one board, before it touches any.
pub(crate) async fn lock_containers<'a>(
    scope: &mut Scope<'_>,
    container_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), StoreError> {
    let ordered: BTreeSet<&str> = container_ids.into_iter().collect();
    for container_id in ordered {
        lock_container(scope, container_id).await?;
    }
    Ok(())
}

pub(crate) async fn delete_member(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM container_members WHERE container_id = ");
    sql.bind(Id(container_id))
        .push_bind(" AND user_id = ", Id(user_id));
    if scope.execute(sql).await? > 0
        && crate::container_team_access::role_for(scope, container_id, user_id)
            .await?
            .is_none()
    {
        // Whoever may no longer see the board is no longer told what changes on it.
        subscriptions::end_subscriptions_to_container(scope, container_id, user_id).await?;
        crate::trash::forget_watcher_of_container(scope, container_id, user_id).await?;
    }
    Ok(())
}

pub(crate) async fn get_builtin_templates(
    scope: &mut Scope<'_>,
) -> Result<Vec<Container>, StoreError> {
    let mut sql = select_containers(scope);
    sql.push_bind(" WHERE is_builtin = ", true)
        .push(" ORDER BY created_at, id");
    fetch_containers(scope, sql).await
}

pub(crate) async fn remove_builtin_templates(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    // The built-in templates are removed for good, past the trash.
    let mut sql = scope.sql(
        "DELETE FROM blocks WHERE container_id IN (SELECT id FROM containers WHERE is_builtin = ",
    );
    sql.bind(true).push(")");
    scope.execute(sql).await?;
    let mut sql = scope.sql("DELETE FROM containers WHERE is_builtin = ");
    sql.bind(true);
    let count = scope.execute(sql).await?;
    tracing::debug!(count, "removed the built-in templates");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_of_the_columns_is_the_columns() {
        assert_eq!(
            CONTAINER_COLUMNS.split(',').count(),
            CONTAINER_COLUMN_COUNT,
            "{CONTAINER_COLUMNS}"
        );
        assert_eq!(
            CONTAINER_SELECT_B.split(',').count(),
            CONTAINER_COLUMN_COUNT,
            "{CONTAINER_SELECT_B}"
        );
    }
}
