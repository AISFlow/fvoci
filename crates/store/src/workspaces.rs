//! Workspaces: the workspaces of an organization.
//!
//! A statement here names the organization itself: the handle's
//! ([`Tenant::Org`](crate::Tenant::Org)). SQLite has no policy that would keep the
//! workspaces of the system organization out of an answer.

use fvoci_model::{
    Block, Container, ContainerKinds, ContainerVisibility, GuestInvitationsChange, OrgKind,
    Workspace, WorkspaceRole,
};

use crate::db::{Bound, FromRow, Id, OptId, Param, Row, Scope, Sql};
use crate::error::StoreError;
use crate::{Store, blocks, container_kinds, container_search, containers, link_locks};

const WORKSPACE_COLUMNS: &str = "id, name, settings, modified_by, updated_at, guest_invitations";

impl Store {
    /// Stores a new workspace of the handle's organization, with the current time as its
    /// creation and modification time.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization,
    /// [`StoreError::InvalidId`] if the workspace's identifier is not one, and
    /// [`StoreError::Database`] if the statement fails, as it does when a workspace with the
    /// identifier exists.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn create_workspace(&self, workspace: &Workspace) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let org = scope.org()?;
            insert_workspace(scope, workspace, org).await
        })
        .await
    }

    /// Turns the invitations of guests to the boards of a workspace on or off, records
    /// who did it, and returns the workspace as it is then. Turned off, the links that
    /// invite guests to its boards are revoked with it, in one step: they do not come
    /// back when it is turned on again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the organization has no such workspace in
    /// use.
    pub async fn set_guest_invitations(
        &self,
        workspace_id: &str,
        change: GuestInvitationsChange,
        modified_by: &str,
    ) -> Result<Workspace, StoreError> {
        self.transaction(async |scope| {
            // Turned off, the workspace's guest links are held all at once, after the
            // organization's row, as a deletion of their boards holds them.
            if !change.enabled {
                link_locks::hold_links_of_organization(scope).await?;
            }
            let now = scope.now_millis();
            // The workspace before its links, as whoever takes somebody out of it has
            // them, and so that a link made meanwhile waits and finds it turned off.
            let mut sql = scope.sql("UPDATE workspaces SET guest_invitations = ");
            sql.bind(change.enabled)
                .push_bind(", modified_by = ", OptId(modified_by))
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?)
                .push(" AND deleted_at IS NULL")
                .push([" RETURNING ", WORKSPACE_COLUMNS]);
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(workspace_not_found)?;
            if !change.enabled {
                let mut sql = scope.sql("UPDATE invitations SET revoked_at = ");
                sql.bind(now.max(1))
                    .push_bind(" WHERE workspace_id = ", Id(workspace_id))
                    .push_bind(" AND org_id = ", scope.org()?)
                    .push_bind(" AND role = ", WorkspaceRole::Guest.as_str())
                    .push_bind(" AND revoked_at = ", 0_i64);
                scope.execute(sql).await?;
            }
            Workspace::from_row(&row)
        })
        .await
    }

    /// Returns the workspace with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the organization has no such workspace.
    pub async fn get_workspace(&self, id: &str) -> Result<Workspace, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_workspaces(scope)?;
            sql.push_bind(" AND id = ", Id(id));
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(workspace_not_found)?;
            Workspace::from_row(&row)
        })
        .await
    }

    /// Returns every workspace of the organization, oldest first. The first is the root
    /// workspace.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_workspaces(&self) -> Result<Vec<Workspace>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_workspaces(scope)?;
            sql.push(" ORDER BY created_at, id");
            scope.all(sql).await
        })
        .await
    }
}

/// Stores a new workspace of the organization `org`.
pub(crate) async fn insert_workspace(
    scope: &mut Scope<'_>,
    workspace: &Workspace,
    org: impl Into<Bound>,
) -> Result<(), StoreError> {
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO workspaces (id, org_id, name, settings, modified_by, guest_invitations, \
         created_at, updated_at) VALUES (",
    );
    sql.bind(Id(&workspace.id))
        .push_bind(", ", org)
        .push_bind(", ", &workspace.title)
        .push_bind(", ", Param::json(&workspace.settings)?)
        .push_bind(", ", OptId(&workspace.modified_by))
        .push_bind(", ", workspace.guest_invitations)
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

impl Store {
    /// Whether the organization this handle works in is the space of one account: what
    /// is in it is its owner's alone, and is told to nobody else.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] for a handle that names no organization,
    /// [`StoreError::NotFound`] if the organization is not there, and
    /// [`StoreError::Corrupt`] if its stored kind is none.
    pub async fn is_own_space(&self) -> Result<bool, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT kind FROM organizations WHERE id = ");
            sql.bind(scope.org()?);
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(|| StoreError::not_found("organization"))?;
            Ok(crate::directory::org_kind(&row, 0)? == OrgKind::Personal)
        })
        .await
    }
}

/// Stores a new organization that people work in together, under the identifier `org`
/// and the given name, with `workspace` as its first workspace, which is its default
/// one. Nobody is a member of either yet.
pub(crate) async fn insert_organization(
    scope: &mut Scope<'_>,
    org: impl Into<Bound>,
    name: &str,
    workspace: &Workspace,
) -> Result<(), StoreError> {
    insert_organization_of(scope, org.into(), name, None, workspace).await
}

/// Stores the space of the account `owner_id`: an organization of the kind `personal`
/// under the identifier `org`, without a name, with `workspace` as its one workspace.
/// The account is no member of either yet.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the account has a space already: one account
/// owns one organization.
pub(crate) async fn insert_own_space(
    scope: &mut Scope<'_>,
    org: impl Into<Bound>,
    owner_id: &str,
    workspace: &Workspace,
) -> Result<(), StoreError> {
    insert_organization_of(scope, org.into(), "", Some(owner_id), workspace).await
}

/// Stores a new organization with its first workspace: the space of `owner`, or one
/// that people work in together.
async fn insert_organization_of(
    scope: &mut Scope<'_>,
    org: Bound,
    name: &str,
    owner: Option<&str>,
    workspace: &Workspace,
) -> Result<(), StoreError> {
    let kind = match owner {
        Some(_) => OrgKind::Personal,
        None => OrgKind::Shared,
    };
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO organizations (id, name, kind, owner_id, settings, created_at, updated_at) \
         VALUES (",
    );
    sql.bind(org.clone())
        .push_bind(", ", name)
        .push_bind(", ", kind.as_str())
        .push_bind(", ", OptId(owner.unwrap_or_default()))
        .push(", '{}'")
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(")");
    scope.execute(sql).await?;
    insert_workspace(scope, workspace, org.clone()).await?;
    let mut sql = scope.sql("UPDATE organizations SET default_workspace_id = ");
    sql.bind(Id(&workspace.id)).push_bind(" WHERE id = ", org);
    scope.execute(sql).await?;
    Ok(())
}

fn workspace_not_found() -> StoreError {
    StoreError::not_found("workspace")
}

/// The workspaces of the handle's organization that are in use.
fn select_workspaces(scope: &Scope<'_>) -> Result<Sql, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        WORKSPACE_COLUMNS,
        " FROM workspaces WHERE org_id = ",
    ]);
    sql.bind(scope.org()?).push(" AND deleted_at IS NULL");
    Ok(sql)
}

impl FromRow for Workspace {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Workspace {
            id: row.id(0)?,
            title: row.text(1)?,
            settings: row.json_map(2, "settings")?,
            modified_by: row.id(3)?,
            updated_at: row.int(4)?,
            guest_invitations: row.bool(5)?,
        })
    }
}

impl Store {
    /// Returns the boards of a workspace whose title matches a search term and that are open
    /// or private with the user as a member with a role, oldest first. Templates are left out.
    ///
    /// An empty `term` matches every board; otherwise the title must contain every
    /// space-separated word of the term, ignoring case.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_containers_for_user_in_workspace(
        &self,
        workspace_id: &str,
        term: &str,
        user_id: &str,
        kinds: ContainerKinds,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| {
            search_containers_for_user_in_workspace(scope, workspace_id, term, user_id, kinds).await
        })
        .await
    }

    /// Returns every board of `kinds` of a workspace that is no template, whoever is a
    /// member of it, oldest first: what the workspace's administrators list.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_containers_of_workspace(
        &self,
        workspace_id: &str,
        kinds: ContainerKinds,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| {
            every_container(scope, workspace_id, Listed::Containers(kinds)).await
        })
        .await
    }

    /// Returns every template of a workspace, whoever is a member of it, oldest first:
    /// what the workspace's administrators list of its templates. The built-in templates
    /// are not among them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_templates_of_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<Container>, StoreError> {
        self.with_conn(async |scope| every_container(scope, workspace_id, Listed::Templates).await)
            .await
    }
}

/// Which boards of a workspace [`every_container`] reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// Those of the kinds that are no template.
    Containers(ContainerKinds),
    /// The templates: boards all of them, since a wiki is none.
    Templates,
}

/// Every board of the workspace that is listed, oldest first.
async fn every_container(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    listed: Listed,
) -> Result<Vec<Container>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        containers::CONTAINER_SELECT_B,
        " FROM containers AS b WHERE b.is_template = ",
    ]);
    sql.bind(listed == Listed::Templates)
        .push_bind(" AND b.workspace_id = ", Id(workspace_id));
    if let Listed::Containers(kinds) = listed {
        container_kinds::push_kinds(&mut sql, "b.kind", kinds);
    }
    sql.push(containers::OLDEST_CONTAINER_FIRST);
    containers::fetch_containers(scope, sql).await
}

async fn search_containers_for_user_in_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    term: &str,
    user_id: &str,
    kinds: ContainerKinds,
) -> Result<Vec<Container>, StoreError> {
    let mut sql = scope.sql([
        "SELECT DISTINCT ",
        containers::CONTAINER_SELECT_B,
        " FROM containers AS b LEFT JOIN container_members AS bm ON b.id = bm.container_id \
         WHERE b.is_template = ",
    ]);
    sql.bind(false)
        .push_bind(" AND b.workspace_id = ", Id(workspace_id));
    container_kinds::push_kinds(&mut sql, "b.kind", kinds);
    sql.push_bind(" AND (b.visibility = ", ContainerVisibility::Open.as_str())
        .push_bind(
            " OR (b.visibility = ",
            ContainerVisibility::Private.as_str(),
        )
        .push_bind(" AND bm.user_id = ", Id(user_id))
        .push(") OR ");
    crate::container_team_access::push_has_team_role(&mut sql, scope, user_id);
    sql.push(")");
    sql.push(containers::OLDEST_CONTAINER_FIRST);
    Ok(container_search::with_title_words(
        containers::fetch_containers(scope, sql).await?,
        term,
    ))
}

impl Store {
    /// The block with the identifier, if its board is of `kinds` and, where a workspace
    /// is named, of that workspace or a built-in template, in one query: a block of
    /// another workspace, or of a board of another kind, is found as one that does not
    /// exist is, and costs the same.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such block and
    /// [`StoreError::Database`] if the query fails.
    pub async fn get_block_within(
        &self,
        block_id: &str,
        workspace_id: Option<&str>,
        kinds: ContainerKinds,
    ) -> Result<Block, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = blocks::select_blocks(scope);
            sql.push_bind(" WHERE id = ", Id(block_id))
                // The block's own board, by its key: what the query costs is the same
                // whether the block is within reach or not.
                .push(
                    " AND EXISTS (SELECT 1 FROM containers AS b WHERE b.id = blocks.container_id",
                );
            container_kinds::push_kinds(&mut sql, "b.kind", kinds);
            if let Some(workspace_id) = workspace_id {
                sql.push_bind(" AND (b.workspace_id = ", Id(workspace_id))
                    .push_bind(" OR b.is_builtin = ", true)
                    .push(")");
            }
            sql.push(")");
            blocks::fetch_blocks(scope, sql)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| StoreError::not_found(format_args!("block ID={block_id}")))
        })
        .await
    }
}

pub(crate) async fn get_template_boards(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<Vec<Container>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        containers::CONTAINER_SELECT_B,
        " FROM containers AS b LEFT JOIN container_members AS bm \
         ON b.id = bm.container_id AND bm.user_id = ",
    ]);
    // Open templates are included even without a membership.
    sql.bind(Id(user_id))
        .push_bind(" WHERE b.is_template = ", true)
        .push_bind(" AND b.is_builtin = ", false)
        .push_bind(" AND b.workspace_id = ", Id(workspace_id))
        .push_bind(
            " AND ((bm.container_id IS NULL AND b.visibility = ",
            ContainerVisibility::Open.as_str(),
        )
        .push(") OR bm.container_id IS NOT NULL OR ");
    crate::container_team_access::push_has_team_role(&mut sql, scope, user_id);
    sql.push(")").push(containers::OLDEST_CONTAINER_FIRST);
    containers::fetch_containers(scope, sql).await
}
