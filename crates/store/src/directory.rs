//! What is asked before the organization of a request is known, and what makes an
//! organization: which workspaces a user has, whose a workspace or a shared board is,
//! which other users a user sees, what a user is in an organization, which sign-in
//! through a provider comes back, what a login page shows of an organization, and which
//! organization the token of an invitation leads into.
//! And whether an object of the file storage is held, which is asked of every
//! organization: an object's place is one across them.
//!
//! Every function here runs one statement, or one short transaction, that reaches the
//! rows of every organization ([`Tenant::System`](crate::Tenant::System)) and is
//! narrowed by what the caller holds: a user's identifier, a read token, the places of
//! objects. Nothing else of the server asks across organizations on behalf of a
//! request.

use std::collections::BTreeSet;

use fvoci_model::{
    ContainerKind, ContainerKinds, ContainerMember, ContainerVisibility, OrgKind, OrgRole, User,
    UserOrganization, UserWorkspace, Workspace, WorkspaceRole,
};
use subtle::ConstantTimeEq as _;

use crate::Store;
use crate::container_kinds;
use crate::container_team_access;
use crate::db::{Dialect, Id, Scope, Sql};
use crate::error::StoreError;
use crate::files;
use crate::loose_objects::LooseState;
use crate::uploads::{OpenUploads, UploadUsage};
use crate::{body_versions, invitations, memberships_everywhere, users, users_by_id};

mod every_organization;
mod session_taken;
mod sign_in;

pub(crate) use session_taken::{HAS_PASSWORD, memberships_of, taken};
pub use session_taken::{Making, SessionTaken};

pub(crate) use every_organization::{
    OpenedWall, close_wall, is_the_instances_organization, of_people, open_wall,
    organizations_made_by,
};

/// The organization that holds what the server ships with and is nobody's: the
/// built-in templates. The schema makes it, under this identifier.
pub const SYSTEM_ORG_ID: &str = "00000000-0000-7000-8000-00000000a000";

/// The workspace of the system organization, which the built-in templates are in.
pub const SYSTEM_WORKSPACE_ID: &str = "00000000-0000-7000-8000-00000000a001";

/// The condition every lookup here ends with: the workspace `t` and its organization
/// `o` are in use, and are not the system's, which is nobody's to work in.
pub(crate) const IN_USE: &str = " AND t.deleted_at IS NULL AND o.deleted_at IS NULL \
                      AND o.is_system = false AND t.is_system = false";

/// The condition that the organization `o`, if it is the space of an account, is of
/// one in use: not of one that waits to be deleted or is being erased.
const OWNER_IN_USE: &str = " AND NOT EXISTS (SELECT 1 FROM users AS owner \
                            WHERE owner.id = o.owner_id AND owner.deleted_at IS NOT NULL)";

/// What a user is in a workspace, and whose the workspace is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAccess {
    /// The organization the workspace belongs to.
    pub org_id: String,
    /// What the user is in that organization; `None` for somebody who is in the
    /// workspace without belonging to the organization, and for a user's own space
    /// (agreeing with [`Store::org_role_of_user`]).
    pub org_role: Option<OrgRole>,
    /// What the user is in the workspace.
    pub workspace_role: WorkspaceRole,
    /// Which sessions of the user open the workspace.
    pub takes: SessionTaken,
    /// What the organization is: one that people work in together, or the user's own
    /// space.
    pub kind: OrgKind,
}

impl WorkspaceAccess {
    /// Whether the user works in the workspace: a member of it who is no guest. A
    /// guest is on the boards they were invited to and reaches those by their board;
    /// what a member may do in the workspace (see its open boards, make a board, join
    /// one) a guest may not.
    #[must_use]
    pub fn works_there(&self) -> bool {
        self.workspace_role != WorkspaceRole::Guest
    }
}

/// Whom a user sees of the other users in a workspace the user is a member of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// The workspace's members, and who is on a board of it with the user: what whoever
    /// works in the workspace sees.
    Workspace,
    /// Who is on a board of the workspace with the user: what a guest sees.
    SharedContainers,
}

/// What a user looks at other users for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looking {
    /// Among the people: a list, a search, the candidates of a mention. Only who is in
    /// the workspace now.
    AmongPeople,
    /// By name: the users named by their identifiers, to show who wrote what. Who left
    /// the workspace too, where they wrote on a board the seer reads.
    ByName,
}

/// The users one user sees: in one workspace, as far as [`Reach`] goes, or across every
/// workspace they are in; and for what ([`Looking`]). The user sees themselves. The
/// rule is the server's `permissions::sees_person`; this is its form in SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sight<'a> {
    /// In the workspace, which the seer is a member of (the caller has asked).
    InWorkspace {
        /// The user who looks.
        seer_id: &'a str,
        /// The workspace the seer looks in.
        workspace_id: &'a str,
        /// How far the seer sees in it.
        reach: Reach,
        /// What for.
        looking: Looking,
    },
    /// Anywhere, by name: who shares with the seer a workspace the seer works in (is no
    /// guest of), or a board of a workspace both are members of, or wrote on a board
    /// the seer reads.
    Anywhere {
        /// The user who looks.
        seer_id: &'a str,
    },
}

/// Pushes the condition that the user `id` is one `sight` sees.
fn push_seen(sql: &mut Sql, scope: &Scope<'_>, sight: Sight<'_>) {
    match sight {
        Sight::InWorkspace {
            seer_id,
            workspace_id,
            reach,
            looking,
        } => {
            // The seer is a member of the workspace, which the store asks too.
            sql.push(
                " AND EXISTS (SELECT 1 FROM workspace_members AS seer WHERE seer.workspace_id = ",
            )
            .bind(Id(workspace_id))
            .push_bind(" AND seer.user_id = ", Id(seer_id))
            .push_bind(") AND (id = ", Id(seer_id))
            // On a board of the workspace with the seer, and in the workspace still.
            .push(
                " OR id IN (SELECT other.user_id FROM container_members AS mine \
                 JOIN containers AS b ON b.id = mine.container_id \
                 JOIN ",
            )
            .push(&container_team_access::directory_members(scope))
            .push(
                " AS other ON other.container_id = mine.container_id \
                 JOIN workspace_members AS them \
                 ON them.workspace_id = b.workspace_id AND them.user_id = other.user_id \
                 WHERE mine.user_id = ",
            )
            .bind(Id(seer_id))
            .push_bind(" AND b.workspace_id = ", Id(workspace_id))
            .push(&scope.same_organization(&[("mine", "b"), ("other", "b"), ("them", "b")]))
            .push(")");
            if reach == Reach::Workspace {
                sql.push(" OR id IN (SELECT wm.user_id FROM workspace_members AS wm WHERE wm.workspace_id = ")
                    .bind(Id(workspace_id))
                    .push(")");
            }
            if looking == Looking::ByName {
                push_writer(sql, scope, seer_id, Some((workspace_id, reach)));
            }
            sql.push(")");
        }
        Sight::Anywhere { seer_id } => {
            sql.push_bind(" AND (id = ", Id(seer_id))
                .push(
                    " OR id IN (SELECT them.user_id FROM workspace_members AS me \
                     JOIN workspace_members AS them ON them.workspace_id = me.workspace_id \
                     JOIN workspaces AS t ON t.id = me.workspace_id \
                     JOIN organizations AS o ON o.id = t.org_id WHERE me.user_id = ",
                )
                .bind(Id(seer_id))
                .push_bind(" AND me.role <> ", WorkspaceRole::Guest.as_str())
                .push(IN_USE)
                .push(&scope.same_organization(&[("me", "t"), ("them", "t")]))
                .push(
                    ") OR id IN (SELECT other.user_id FROM container_members AS mine \
                     JOIN containers AS b ON b.id = mine.container_id \
                     JOIN workspace_members AS me \
                     ON me.workspace_id = b.workspace_id AND me.user_id = mine.user_id \
                     JOIN workspaces AS t ON t.id = b.workspace_id \
                     JOIN organizations AS o ON o.id = t.org_id \
                     JOIN ",
                )
                .push(&container_team_access::directory_members(scope))
                .push(
                    " AS other ON other.container_id = mine.container_id \
                     JOIN workspace_members AS them \
                     ON them.workspace_id = b.workspace_id AND them.user_id = other.user_id \
                     WHERE mine.user_id = ",
                )
                .bind(Id(seer_id))
                .push(IN_USE)
                .push(&scope.same_organization(&[
                    ("mine", "b"),
                    ("b", "t"),
                    ("me", "t"),
                    ("other", "b"),
                    ("them", "b"),
                ]))
                .push(")");
            push_writer(sql, scope, seer_id, None);
            sql.push(")");
        }
    }
}

/// Pushes `OR` the condition that the user `id` authored a board, a block, a kept
/// comment version, a kept version of a card's body or a version of a file the seer
/// reads, as
/// `permissions::grant` has it: one they hold a
/// role on, or an open template or an open wiki of a workspace they work in; of the
/// workspace `within`
/// names, as far as its [`Reach`] goes, or of any workspace in use the seer is a member
/// of.
fn push_writer(sql: &mut Sql, scope: &Scope<'_>, seer_id: &str, within: Option<(&str, Reach)>) {
    sql.push(
        " OR EXISTS (SELECT 1 FROM containers AS b \
         JOIN workspace_members AS me ON me.workspace_id = b.workspace_id \
         JOIN workspaces AS t ON t.id = b.workspace_id \
         JOIN organizations AS o ON o.id = t.org_id \
         WHERE me.user_id = ",
    )
    .bind(Id(seer_id))
    .push(IN_USE)
    .push(&scope.same_organization(&[("b", "t"), ("me", "t")]))
    .push(" AND (b.created_by = users.id")
    .push(&crate::file_versions::or_wrote_file_of_container(scope))
    .push(" OR EXISTS (SELECT 1 FROM blocks bl WHERE bl.container_id = b.id")
    .push(&scope.same_organization(&[("bl", "b")]))
    .push(
        " AND (bl.created_by = users.id OR bl.modified_by = users.id \
         OR (bl.type = 'comment' AND EXISTS (SELECT 1 FROM comment_versions cv \
         WHERE cv.block_id = bl.id AND cv.written_by = users.id",
    )
    .push(&scope.same_organization(&[("cv", "bl")]))
    .push(")) OR (")
    .push(&body_versions::with_room_sql("bl.type"))
    .push(
        " AND EXISTS (SELECT 1 FROM body_version_editors ve \
         WHERE ve.block_id = bl.id AND ve.version > 0 AND ve.user_id = users.id",
    )
    .push(&scope.same_organization(&[("ve", "bl")]))
    .push(
        ")))) OR EXISTS (SELECT 1 FROM deleted_blocks d \
         JOIN comment_versions cv ON cv.block_id = d.id \
         WHERE d.container_id = b.id AND cv.written_by = users.id",
    )
    .push(&scope.same_organization(&[("d", "b"), ("cv", "d")]))
    .push(" AND ")
    .push(match scope.dialect() {
        Dialect::Sqlite => "json_extract(d.block, '$.type') = 'comment'",
        Dialect::Postgres => "d.block->>'type' = 'comment'",
    })
    .push("))");
    let open_where_seer_works = match within {
        Some((workspace_id, reach)) => {
            sql.push_bind(" AND b.workspace_id = ", Id(workspace_id));
            reach == Reach::Workspace
        }
        None => true,
    };
    sql.push(
        " AND (EXISTS (SELECT 1 FROM container_members AS mine WHERE mine.container_id = b.id \
         AND mine.user_id = me.user_id",
    )
    .push(&scope.same_organization(&[("mine", "b")]))
    .push(")");
    sql.push(" OR ");
    container_team_access::push_has_team_role(sql, scope, seer_id);
    if open_where_seer_works {
        // Read without a role: an open template, an open wiki, and an open course, for
        // who works in its workspace. An open board that is none of these is joined to be
        // read.
        sql.push_bind(" OR (b.visibility = ", ContainerVisibility::Open.as_str())
            .push_bind(" AND (b.is_template = ", true)
            .push(" OR b.kind IN (")
            .bind_list(
                ContainerKinds::OPEN_TO_READ
                    .iter()
                    .map(ContainerKind::as_str),
            )
            .push_bind(")) AND me.role <> ", WorkspaceRole::Guest.as_str())
            .push(")");
    }
    sql.push("))");
}

impl Store {
    /// The users `sight` takes in whose login name, display name or a part of whose
    /// name (either way round, and the name they wrote) contains `search_query`,
    /// ignoring case: at most ten, the oldest accounts first; for an empty query the ten
    /// oldest of them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn search_users_in_sight(
        &self,
        sight: Sight<'_>,
        search_query: &str,
    ) -> Result<Vec<User>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                users::search_users(scope, search_query, |sql, scope| {
                    push_seen(sql, scope, sight);
                })
                .await
            })
            .await
    }

    /// The users with the given identifiers that `sight` takes in, oldest first, each
    /// once. A user it does not take in is left out, as one that does not exist: the
    /// answer does not say which identifiers found nobody, nor why.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_users_in_sight(
        &self,
        sight: Sight<'_>,
        user_ids: &[String],
    ) -> Result<Vec<User>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                users_by_id::found_users_by_ids(scope, user_ids, |sql, scope| {
                    push_seen(sql, scope, sight);
                })
                .await
            })
            .await
    }

    /// Makes an organization with the given name and its first workspace, which is its
    /// default one, and returns the organization's identifier. Nobody is a member of
    /// either yet.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if the workspace's identifier is not one, and
    /// [`StoreError::Database`] if a statement fails, as it does on an SQLite database
    /// that holds an organization already.
    pub async fn create_organization(
        &self,
        name: &str,
        workspace: &Workspace,
    ) -> Result<String, StoreError> {
        self.system()
            .transaction(async |scope| create_organization(scope, name, workspace).await)
            .await
    }

    /// The instance's own organization: the oldest non-system organization in use that
    /// people work in together.
    /// This identifies the organization whose administrators follow instance roles;
    /// it does not select an organization for requests or create one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn instance_organization(&self) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| oldest_organization(scope).await)
            .await
    }

    /// Returns a pre-existing shared organization when the instance has not yet had its
    /// first user (`bootstrapped = false`), or `None` once bootstrapped or on a fresh
    /// multi-organization database.
    pub(crate) async fn unclaimed_instance_organization(
        &self,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT o.id FROM organizations AS o \
                     JOIN instance AS i ON i.id = 1 \
                     WHERE i.bootstrapped = ",
                );
                sql.bind(false)
                    .push_bind(" AND o.is_system = ", false)
                    .push(" AND o.deleted_at IS NULL");
                of_people(&mut sql, "o.kind");
                sql.push(" ORDER BY o.created_at, o.id LIMIT 1");
                scope
                    .fetch_optional(sql)
                    .await?
                    .map(|row| row.id(0))
                    .transpose()
            })
            .await
    }

    /// The organization that is the space of the account `user_id`, in use or not;
    /// `None` for an account that has none. An account's space is the same one for as
    /// long as the account is there.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub(crate) async fn space_of(&self, user_id: &str) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| space_of(scope, user_id).await)
            .await
    }

    /// What the user is in the workspace, and whose the workspace is; `None` if the
    /// user is no member of it, whether it exists or not. The user is the one of a
    /// session: whether the account is still in use is the session's to say.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if a stored role is none.
    pub async fn resolve_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceAccess>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql([
                    "SELECT t.org_id, wm.role, om.role, p.id, o.kind, ",
                    HAS_PASSWORD,
                    " FROM workspace_members AS wm \
                     JOIN users AS u ON u.id = wm.user_id \
                     JOIN workspaces AS t ON t.id = wm.workspace_id \
                     JOIN organizations AS o ON o.id = t.org_id \
                     LEFT JOIN organization_members AS om \
                     ON om.org_id = t.org_id AND om.user_id = wm.user_id \
                     LEFT JOIN oidc_providers AS p ON p.org_id = t.org_id AND p.sso_required = ",
                ]);
                sql.bind(true)
                    .push_bind(" WHERE wm.user_id = ", Id(user_id))
                    .push_bind(" AND wm.workspace_id = ", Id(workspace_id))
                    .push(IN_USE)
                    .push(&scope.same_organization(&[("wm", "t")]));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(None);
                };
                let org_role = row.text(2)?;
                let kind = org_kind(&row, 4)?;
                Ok(Some(WorkspaceAccess {
                    org_id: row.id(0)?,
                    workspace_role: role(&row, 1)?,
                    org_role: if kind == OrgKind::Personal || org_role.is_empty() {
                        None
                    } else {
                        Some(role(&row, 2)?)
                    },
                    takes: taken(kind, row.id(3)?, &row.text(5)?),
                    kind,
                }))
            })
            .await
    }

    /// The workspaces the user is a member of, in every organization: by organization,
    /// the oldest first, and in each the oldest workspace first; the workspace of the
    /// user's own space last, whenever it was made. `making` is that of the asking
    /// session: a workspace of an organization that takes only sessions made through
    /// another provider says so (`sso_required`), and the user's own space says that
    /// it takes a session that has shown the password (`password_required`); both are
    /// listed all the same.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if a stored role is none.
    pub async fn workspaces_of_user(
        &self,
        user_id: &str,
        making: Making<&str>,
    ) -> Result<Vec<UserWorkspace>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let memberships = memberships_of(scope, user_id).await?;
                Ok(memberships
                    .into_iter()
                    .map(|membership| membership.listed_for(making))
                    .collect())
            })
            .await
    }

    /// The provider a session of the user must be made through to open what is the
    /// organization's: `Some` if the organization takes no other session and the user
    /// has a place in it, as a member of it or of one of its workspaces in use. `None`
    /// for an organization that takes a password too or is not in use, and for a user
    /// who has no place in it, whether it exists or not: who has nothing there to open
    /// is told nothing of it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn required_provider_for(
        &self,
        user_id: &str,
        org_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT p.id FROM oidc_providers AS p \
                     JOIN organizations AS o ON o.id = p.org_id WHERE p.org_id = ",
                );
                sql.bind(Id(org_id))
                    .push_bind(" AND p.sso_required = ", true)
                    .push(
                        " AND o.deleted_at IS NULL AND o.is_system = false \
                         AND (EXISTS (SELECT 1 FROM organization_members AS om \
                         WHERE om.org_id = p.org_id AND om.user_id = ",
                    )
                    .bind(Id(user_id))
                    .push(
                        ") OR EXISTS (SELECT 1 FROM workspace_members AS wm \
                         JOIN workspaces AS t ON t.id = wm.workspace_id \
                         WHERE t.org_id = p.org_id AND t.deleted_at IS NULL AND wm.user_id = ",
                    )
                    .bind(Id(user_id))
                    .push("))");
                scope
                    .fetch_optional(sql)
                    .await?
                    .map(|row| row.id(0))
                    .transpose()
            })
            .await
    }

    /// Returns the upload sessions open at `now`, all users' together and of every
    /// organization, as [`Store::upload_usage`] counts them for the server: a number
    /// of the whole server, for its metrics.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn open_uploads(&self, now: i64) -> Result<OpenUploads, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT CAST(COUNT(*) AS BIGINT), CAST(COALESCE(SUM(size), 0) AS BIGINT) \
                 FROM uploads WHERE closed_at = 0 AND expires_at > ",
                );
                sql.bind(now);
                let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
                Ok(OpenUploads {
                    sessions: row.int(0)?,
                    bytes: row.int(1)?,
                })
            })
            .await
    }

    /// Returns what the open upload sessions hold back at `now`: those that are not
    /// closed and have not ended. A session that was closed and is not thrown away yet,
    /// as after a crash, does not count.
    ///
    /// The limits these figures are for are the user's and the server's, not an
    /// organization's: the sessions of every organization are counted, the user's own
    /// by the user's identifier. Nothing of another's session is told but its size in
    /// the server's sum.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn upload_usage(&self, user_id: &str, now: i64) -> Result<UploadUsage, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql("SELECT CAST(COALESCE(SUM(CASE WHEN user_id = ");
                sql.bind(Id(user_id))
                .push(" THEN 1 ELSE 0 END), 0) AS BIGINT), CAST(COALESCE(SUM(CASE WHEN user_id = ")
                .bind(Id(user_id))
                .push(
                    " THEN size ELSE 0 END), 0) AS BIGINT), CAST(COALESCE(SUM(size), 0) AS BIGINT) \
                     FROM uploads WHERE closed_at = 0 AND expires_at > ",
                )
                .bind(now);
                let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
                Ok(UploadUsage {
                    user_sessions: row.int(0)?,
                    user_bytes: row.int(1)?,
                    server_bytes: row.int(2)?,
                })
            })
            .await
    }

    /// Whether a row of any organization names a place of the file storage otherwise
    /// than the one way: a record, of a board or of one in the trash, a listed object,
    /// or an upload session. Asked once a process: the store takes no such row since
    /// it has the rule, and while one is there nothing is removed
    /// ([`Store::held_objects`]). One found is logged with its place.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn other_spellings_stored(&self) -> Result<bool, StoreError> {
        let found = self
            .inner
            .other_spellings
            .get_or_try_init(async || {
                let found = self.system().with_conn(files::other_spellings).await?;
                if let Some(path) = found.first() {
                    tracing::error!(
                        count = found.len(),
                        path,
                        "rows name places of the file storage otherwise than the one way; \
                         no object is removed until they are written the one way"
                    );
                }
                Ok::<_, StoreError>(!found.is_empty())
            })
            .await?;
        Ok(*found)
    }

    /// The places among `paths` whose object something of any organization holds: a
    /// file's record, of a board or of a board in the trash, an object being written
    /// there, or an upload session that makes its file there, but for `except_upload`.
    /// Whatever a row says of its board or organization, an object it names is not
    /// removed under it: whether one may go is asked here, of every organization,
    /// before it is removed.
    ///
    /// A place is compared as it is spelled, which is the one way the storage takes
    /// ([`fvoci_model::is_storage_path`]). One among `paths` spelled otherwise is held:
    /// what another spelling would name is not known, and nothing is removed under it.
    /// And while a row stored before that rule names a place otherwise (asked once a
    /// process), every place is held.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn held_objects(
        &self,
        paths: &[&str],
        except_upload: Option<&str>,
    ) -> Result<BTreeSet<String>, StoreError> {
        let (paths, unknown): (Vec<&str>, Vec<&str>) = paths
            .iter()
            .copied()
            .partition(|path| fvoci_model::is_storage_path(path));
        let mut held: BTreeSet<String> = unknown.into_iter().map(str::to_owned).collect();
        if paths.is_empty() {
            return Ok(held);
        }
        if self.other_spellings_stored().await? {
            held.extend(paths.into_iter().map(str::to_owned));
            return Ok(held);
        }
        self.system()
            .with_conn(async |scope| {
                let (trashed_path, trashed_files) = match scope.dialect() {
                    Dialect::Sqlite => {
                        ("json_extract(f.value, '$.path')", "json_each(d.files) AS f")
                    }
                    Dialect::Postgres => (
                        "f.value ->> 'path'",
                        "json_array_elements(d.files) AS f(value)",
                    ),
                };
                let mut sql = scope.sql("SELECT path FROM files WHERE path IN (");
                sql.bind_list(paths.iter().copied())
                    .push([
                        ") UNION SELECT ",
                        trashed_path,
                        " FROM deleted_containers AS d, ",
                        trashed_files,
                        " WHERE ",
                        trashed_path,
                        " IN (",
                    ])
                    .bind_list(paths.iter().copied())
                    .push(") UNION SELECT path FROM file_versions WHERE path IN (")
                    .bind_list(paths.iter().copied())
                    .push(") UNION SELECT path FROM loose_objects WHERE path IN (")
                    .bind_list(paths.iter().copied())
                    .push_bind(") AND state = ", LooseState::Writing.as_str())
                    .push(" UNION SELECT path FROM uploads WHERE path IN (")
                    .bind_list(paths.iter().copied())
                    .push(")");
                if let Some(upload_id) = except_upload {
                    sql.push_bind(" AND id <> ", upload_id);
                }
                for row in scope.fetch_all(sql).await? {
                    held.insert(row.text(0)?);
                }
                Ok(held)
            })
            .await
    }

    /// The user's memberships of boards of `kinds` in live workspaces of every
    /// organization, direct and through a team (`inherited_role`), ordered by board
    /// identifier. Only the named user's memberships are returned, and only of the
    /// workspaces a session of that `making` opens ([`SessionTaken::opened_by`]): those
    /// of an organization that takes only sessions of another provider are left out,
    /// and those of the user's own space that such a session does not open.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn container_memberships_of_user(
        &self,
        user_id: &str,
        making: Making<&str>,
        kinds: ContainerKinds,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                memberships_everywhere::of_user(scope, user_id, making, kinds).await
            })
            .await
    }

    /// The board of a live block in the named workspace, for the holder of its read
    /// token. The lookup reaches across organizations but returns nothing without a
    /// valid public link to that board.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if either lookup fails.
    pub async fn board_of_shared_block(
        &self,
        workspace_id: &str,
        block_id: &str,
        read_token: &str,
    ) -> Result<Option<String>, StoreError> {
        let container_id = self
            .system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT bl.container_id FROM blocks AS bl JOIN containers AS b ON b.id = bl.container_id \
                 WHERE bl.id = ",
                );
                sql.bind(Id(block_id))
                    .push_bind(" AND b.workspace_id = ", Id(workspace_id))
                    .push(&scope.same_organization(&[("bl", "b")]));
                scope
                    .fetch_optional(sql)
                    .await?
                    .map(|row| row.id(0))
                    .transpose()
            })
            .await?;
        let Some(container_id) = container_id else {
            return Ok(None);
        };
        // The blocks a token follows are a board's: a wiki's token reads its pages only.
        let shared = self.shared_container(&container_id, read_token).await?;
        Ok(shared
            .filter(|shared| shared.kind == ContainerKind::Board)
            .map(|_| container_id))
    }

    /// The board or the wiki that is shared, for whoever holds its read token: its
    /// organization and its kind, which says what the token reads of it (a board's
    /// cards, a wiki's pages). `None` if it is not shared, is not there, or the token
    /// is not its own. Nothing of the system organization is shared, nothing of a
    /// workspace or an organization that was deleted, nothing of an organization
    /// that turned its public links off, and nothing of the space of an account that
    /// is out of use: one that waits to be deleted has its links closed meanwhile, and
    /// they open again if it takes the deletion back.
    ///
    /// The tokens are compared without showing through timing where they differ.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn shared_container(
        &self,
        container_id: &str,
        read_token: &str,
    ) -> Result<Option<SharedContainer>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT t.org_id, s.token, b.kind FROM public_links AS s \
                     JOIN containers AS b ON b.id = s.container_id \
                     JOIN workspaces AS t ON t.id = b.workspace_id \
                     JOIN organizations AS o ON o.id = t.org_id WHERE s.container_id = ",
                );
                sql.bind(Id(container_id))
                    .push_bind(" AND s.enabled = ", true)
                    .push_bind(" AND o.public_links = ", true)
                    .push(IN_USE)
                    .push(OWNER_IN_USE)
                    .push(&scope.same_organization(&[("s", "b"), ("b", "t")]));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(None);
                };
                let token = row.text(1)?;
                // A board that is not shared has no token; nothing matches that, not
                // even nothing.
                let held: bool = token.as_bytes().ct_eq(read_token.as_bytes()).into();
                if token.is_empty() || !held {
                    return Ok(None);
                }
                let kind = container_kinds::kind_from_text(&row.text(2)?)?;
                if !ContainerKinds::LINKED.holds(kind) {
                    return Ok(None);
                }
                Ok(Some(SharedContainer {
                    org_id: row.id(0)?,
                    kind,
                }))
            })
            .await
    }
}

/// A board or a wiki that a read token opens ([`Store::shared_container`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedContainer {
    /// The organization it is of, which it is read in.
    pub org_id: String,
    /// Which of the two it is.
    pub kind: ContainerKind,
}

impl Store {
    /// The organization of the live link whose token has the digest `token_hash`, at
    /// `now`: one that is not revoked, has not ended and was not used as often as it
    /// may be, into a workspace and an organization in use. `None` for any other, and
    /// for no link at all: one read, the same whatever is wrong with a link. The use a
    /// registration made from the browser whose marker has the digest `replacing`
    /// took of the link does not count: what the browser does next replaces it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn org_of_live_invitation(
        &self,
        token_hash: &str,
        now: i64,
        replacing: Option<&str>,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT i.org_id FROM invitations AS i \
                     JOIN workspaces AS t ON t.id = i.workspace_id \
                     JOIN organizations AS o ON o.id = t.org_id WHERE i.token_hash = ",
                );
                sql.bind(token_hash);
                invitations::push_live_but_for(&mut sql, now, replacing);
                invitations::push_maker_in_use(&mut sql);
                sql.push(IN_USE)
                    .push(&scope.same_organization(&[("i", "t")]));
                match scope.fetch_optional(sql).await? {
                    Some(row) => row.id(0).map(Some),
                    None => Ok(None),
                }
            })
            .await
    }

    /// The workspaces of every organization that were put in the trash before
    /// `deleted_before`, by organization and identifier: what the housekeeping removes
    /// for good, each in its organization ([`Store::purge_workspace`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn workspaces_to_purge(
        &self,
        deleted_before: i64,
    ) -> Result<Vec<(String, String)>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT org_id, id FROM workspaces WHERE is_system = false \
                     AND deleted_at IS NOT NULL AND deleted_at < ",
                );
                sql.bind(deleted_before).push(" ORDER BY org_id, id");
                scope
                    .fetch_all(sql)
                    .await?
                    .iter()
                    .map(|row| Ok((row.id(0)?, row.id(1)?)))
                    .collect()
            })
            .await
    }

    /// What the user is in the organization; `None` for somebody who is no member, and
    /// for an organization that is not in use or is the system's, whether it exists or
    /// not. And for a user's own space, its owner's too: nobody is anything in it to
    /// the operations of an organization, which ask here who administers one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if the stored role is none.
    pub async fn org_role_of_user(
        &self,
        user_id: &str,
        org_id: &str,
    ) -> Result<Option<OrgRole>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT om.role FROM organization_members AS om \
                     JOIN organizations AS o ON o.id = om.org_id WHERE om.user_id = ",
                );
                sql.bind(Id(user_id))
                    .push_bind(" AND om.org_id = ", Id(org_id))
                    .push(" AND o.deleted_at IS NULL AND o.is_system = false")
                    .push_bind(" AND o.kind = ", OrgKind::Shared.as_str());
                match scope.fetch_optional(sql).await? {
                    Some(row) => role(&row, 0).map(Some),
                    None => Ok(None),
                }
            })
            .await
    }

    /// Every organization in use that people work in together of which `user_id` is a
    /// member, oldest first: includes organizations where the account is in no
    /// workspace, and leaves out personal spaces and organizations where it is only a
    /// workspace guest.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if a stored role is none.
    pub async fn organizations_of_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<UserOrganization>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                every_organization::organizations_of_user(scope, user_id).await
            })
            .await
    }
}

use every_organization::{create_organization, role};
pub(crate) use every_organization::{oldest_organization, org_kind, space_of};
