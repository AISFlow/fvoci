//! Invitations: the links that let somebody into a workspace, as a member of it or as a
//! guest of one of its boards.
//!
//! A link is found by the digest of its token; the token itself is never here. Every
//! statement names the organization itself: the handle's
//! ([`Tenant::Org`](crate::Tenant::Org)). Which organization a token leads into is asked
//! in `directory.rs`.
//!
//! A link lets somebody in while it is live: not revoked, not ended, and not used as
//! often as it may be. Whether who made it may still invite is the server's to ask,
//! of what the store answers about that user as about any other; an acceptance is
//! stored only if the maker is still what the server found them to be then
//! ([`Inviter`]).

use fvoci_model::{
    AuditAction, ContainerMember, ContainerRole, Invitation, OrgRole, User, WorkspaceRole, new_id,
};

use crate::accounts::{self, Which};
use crate::audit::{self, AuditEntry};
use crate::containers::{find_member, save_member};
use crate::db::{FromRow, Id, LockRank, OptId, Param, Row, RowLock, Scope, Sql};
use crate::error::{StoreError, Taken};
use crate::link_locks::{self, Turn};
use crate::trash::{self, TrashedRoles};
use crate::{Standing, Store, signups};
use fvoci_model::InstanceRole;

/// The columns of an invitation, in the order [`Invitation::from_row`] reads them.
const COLUMNS: &str = "id, workspace_id, role, container_id, container_role, created_by, created_at, \
                       expires_at, max_uses, uses, parent_id, recipient";
/// [`COLUMNS`] of the table as `i`.
pub(crate) const COLUMNS_OF_I: &str = "i.id, i.workspace_id, i.role, i.container_id, i.container_role, \
                            i.created_by, i.created_at, i.expires_at, i.max_uses, i.uses, \
                            i.parent_id, i.recipient";

/// A link to store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewLink<'a> {
    /// The workspace the link leads into.
    pub workspace_id: &'a str,
    /// For a guest: the board they are invited to, and their role on it. `None` for a
    /// link that makes a member of the workspace.
    pub guest_of: Option<(&'a str, ContainerRole)>,
    /// The digest of the link's token.
    pub token_hash: &'a str,
    /// The user who makes the link.
    pub created_by: &'a str,
    /// When the link is made, in milliseconds since the Unix epoch.
    pub now: i64,
    /// When the link ends, in milliseconds since the Unix epoch.
    pub expires_at: i64,
    /// How often the link may be used; `None` for as often as it is until it ends.
    pub max_uses: Option<i64>,
    /// For a link made for one recipient of a mailed link: the link it is made like,
    /// of the same workspace, and the address it is mailed to.
    pub mailed: Option<MailedTo<'a>>,
}

/// Where a link made for one recipient of a mailed link comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MailedTo<'a> {
    /// The link it is made like.
    pub parent_id: &'a str,
    /// The address it is mailed to.
    pub recipient: &'a str,
}

impl std::fmt::Debug for MailedTo<'_> {
    /// Leaves out the address.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailedTo")
            .field("parent_id", &self.parent_id)
            .finish_non_exhaustive()
    }
}

/// Who accepts a link.
#[derive(Debug)]
pub enum Acceptor<'a> {
    /// The account with this identifier.
    Account(&'a str),
    /// A new account, stored with the acceptance and updated to what was stored.
    NewAccount(&'a mut User),
    /// A new account, made from the browser of a registration that waits for its
    /// address, which it replaces.
    Replacing {
        /// The account, stored with the acceptance and updated to what was stored.
        user: &'a mut User,
        /// The digest of the marker of the browser's registration: removed with the
        /// acceptance, its name and address free for the account and its use of a
        /// link given back, of this link too.
        browser: &'a str,
    },
}

/// Who made a link, as the server found them when it found that their link lets
/// somebody in still. The acceptance is stored only if they are that when it is
/// written.
#[derive(Debug, Clone, Copy)]
pub struct Inviter<'a> {
    /// Their role in the link's workspace and, for a guest's link, their membership of
    /// its board.
    pub standing: Standing<'a>,
    /// What they are to the board of a guest's link.
    pub of_container: InviterOfContainer,
}

/// What the maker of a guest's link is to its board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviterOfContainer {
    /// On it, or the link is for members and has no board.
    Member,
    /// An administrator of the workspace who manages the board and does not read it:
    /// whom their link puts on the board is recorded in the audit log, as whom they
    /// add to it themselves is.
    ManagerOnly,
}

/// What became of the acceptance of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// Somebody was let in.
    Accepted(Accepted),
    /// There is no such live link: nothing was stored.
    Unavailable,
    /// The maker of the link is not what [`Inviter`] says: nothing was stored. Whether
    /// the link lets anybody in still is to be asked again.
    InviterChanged,
}

impl Acceptance {
    /// Where somebody was let in, if anybody was.
    #[must_use]
    pub fn accepted(self) -> Option<Accepted> {
        match self {
            Self::Accepted(accepted) => Some(accepted),
            Self::Unavailable | Self::InviterChanged => None,
        }
    }
}

/// Where an accepted link let somebody in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// The account that was let in.
    pub user_id: String,
    /// The workspace the link let them into.
    pub workspace_id: String,
    /// For a guest's link: the board.
    pub container_id: Option<String>,
    /// Whether the acceptance changed anything, and so counts as a use of the link:
    /// not if the account had already all the link gives.
    pub used: bool,
}

impl Store {
    /// Stores a link into a workspace in use of the handle's organization, and returns
    /// it as stored. A guest's link names a board of that workspace that is no template.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the organization has no such workspace in
    /// use, or the workspace no such board or invites no guests to it for a guest's
    /// link, or the link it is made like is revoked or ended, or of another board;
    /// [`StoreError::NoOrganization`] if the handle names none; and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn create_invitation(&self, link: &NewLink<'_>) -> Result<Invitation, StoreError> {
        self.transaction(async |scope| {
            link_locks::hold_for_new_link(scope, link).await?;
            let (role, container_id, container_role) = match link.guest_of {
                Some((container_id, container_role)) => {
                    (WorkspaceRole::Guest, container_id, Some(container_role.as_str()))
                }
                None => (WorkspaceRole::Member, "", None),
            };
            let mut sql = scope.sql(
                "INSERT INTO invitations (id, org_id, workspace_id, role, container_id, container_role, \
                 token_hash, created_by, created_at, expires_at, max_uses, parent_id, \
                 recipient) VALUES (",
            );
            sql.bind(Id(&new_id()))
                .push_bind(", ", scope.org()?)
                .push_bind(", ", Id(link.workspace_id))
                .push_bind(", ", role.as_str())
                .push_bind(", ", OptId(container_id))
                .push_bind(", ", Param::OptText(container_role.map(str::to_owned)))
                .push_bind(", ", link.token_hash)
                .push_bind(", ", Id(link.created_by))
                .push_bind(", ", link.now)
                .push_bind(", ", link.expires_at)
                .push_bind(", ", Param::OptInt(link.max_uses))
                .push_bind(
                    ", ",
                    OptId(link.mailed.map_or("", |mailed| mailed.parent_id)),
                )
                .push_bind(
                    ", ",
                    Param::OptText(link.mailed.map(|mailed| mailed.recipient.to_owned())),
                )
                .push([") RETURNING ", COLUMNS]);
            // An insert that returns no row has failed already.
            scope.first(sql).await?.ok_or(StoreError::NoRows)
        })
        .await
    }

    /// The live link of the handle's organization whose token has the digest
    /// `token_hash`, if there is one at `now`; live also if only the registration made
    /// from the browser whose marker has the digest `replacing` uses it up: what the
    /// browser does next replaces that registration.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn live_invitation(
        &self,
        token_hash: &str,
        now: i64,
        replacing: Option<&str>,
    ) -> Result<Option<Invitation>, StoreError> {
        self.with_conn(async |scope| {
            let sql = select_live_by_token(scope, token_hash, now, replacing)?;
            scope.first(sql).await
        })
        .await
    }

    /// The links into a workspace of the handle's organization listed at `now` (the
    /// live ones, and a used one while an unused child of it is live), the newest
    /// first: all of them, or those `created_by` made.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn live_invitations(
        &self,
        workspace_id: &str,
        created_by: Option<&str>,
        now: i64,
    ) -> Result<Vec<Invitation>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_listed(scope, now)?;
            sql.push_bind(" AND i.workspace_id = ", Id(workspace_id));
            if let Some(user_id) = created_by {
                sql.push_bind(" AND i.created_by = ", Id(user_id));
            }
            sql.push(" ORDER BY i.created_at DESC, i.id DESC");
            scope.all(sql).await
        })
        .await
    }

    /// Revokes a link into a workspace of the handle's organization that is neither
    /// revoked nor ended, used or not, any of them or one `created_by` made; says
    /// whether there was such a link. A revoked link
    /// lets nobody in from then on, and nor do the links made like it for the
    /// recipients of its mail that nobody used yet, whoever made them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn revoke_invitation(
        &self,
        workspace_id: &str,
        invitation_id: &str,
        created_by: Option<&str>,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.transaction(async |scope| {
            // The link and its children: against an acceptance of a child that replaces
            // a registration by the link, and a deletion of their board.
            link_locks::hold_links_of_organization(scope).await?;
            let mut sql = scope.sql("UPDATE invitations AS i SET revoked_at = ");
            sql.bind(now.max(1))
                .push_bind(" WHERE i.id = ", Id(invitation_id))
                .push_bind(" AND i.workspace_id = ", Id(workspace_id))
                .push_bind(" AND i.org_id = ", scope.org()?)
                // A used link may still have children to go with it.
                .push_bind(" AND i.revoked_at = ", 0_i64)
                .push_bind(" AND i.expires_at > ", now);
            if let Some(user_id) = created_by {
                sql.push_bind(" AND i.created_by = ", Id(user_id));
            }
            if scope.execute(sql).await? == 0 {
                return Ok(false);
            }
            let mut sql = scope.sql("UPDATE invitations AS i SET revoked_at = ");
            sql.bind(now.max(1))
                .push_bind(" WHERE i.parent_id = ", Id(invitation_id))
                .push_bind(" AND i.org_id = ", scope.org()?)
                .push_bind(" AND i.uses = ", 0_i64);
            push_live(&mut sql, now);
            scope.execute(sql).await?;
            Ok(true)
        })
        .await
    }

    /// The link of the handle's organization whose token has the digest `token_hash`,
    /// if it is neither revoked nor ended at `now`, however often it was used: what the
    /// links of a mailed guest link are made like, which go out whether or not the
    /// link itself was used.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn unended_invitation(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<Invitation>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                COLUMNS_OF_I,
                " FROM invitations AS i WHERE i.org_id = ",
            ]);
            sql.bind(scope.org()?)
                .push_bind(" AND i.token_hash = ", token_hash)
                .push_bind(" AND i.revoked_at = ", 0_i64)
                .push_bind(" AND i.expires_at > ", now);
            scope.first(sql).await
        })
        .await
    }

    /// Lets `acceptor` in by the live link of the handle's organization whose token
    /// has the digest `token_hash`, in one transaction, and counts the use; nothing is
    /// stored if there is no such link at `now`. Acceptances at once of one link wait
    /// for each other, so that it is used no more often than it may be.
    ///
    /// The maker of the link is looked at again once the link is held, after whatever
    /// this waited for: if they are not what `inviter` says (taken out of the workspace
    /// meanwhile, say), nothing is stored. Their membership of the workspace is held
    /// from then to the end, so that who takes them out waits for this.
    ///
    /// - A member's link makes the account a member of the organization and of the
    ///   workspace: who was a guest of the workspace becomes a member, and who works
    ///   there stays what they are.
    /// - A guest's link makes the account a member of its board, in its role: who was
    ///   no member of the workspace becomes a guest of it, and holds no other role on
    ///   its boards, those in the trash too; who was one stays what they are in it, and
    ///   keeps a higher role they have on the board. It lets nobody in while its
    ///   workspace invites no guests, or once its board is gone.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if a new account's name or email address is
    /// taken, or ([`Taken::LinkUse`]) the link was used as often as it may be after
    /// all; [`StoreError::NoOrganization`] if the handle names none; and
    /// [`StoreError::Database`] if a statement fails, as it does for an account that
    /// does not exist. Nothing is stored then.
    pub async fn accept_invitation(
        &self,
        token_hash: &str,
        acceptor: Acceptor<'_>,
        now: i64,
        inviter: Inviter<'_>,
    ) -> Result<Acceptance, StoreError> {
        self.transaction(async |scope| {
            // The link of what is replaced may be another than this one.
            let replacing = match &acceptor {
                Acceptor::Replacing { browser, .. } => {
                    Some((*browser, link_locks::take_turn(scope).await?))
                }
                Acceptor::Account(_) | Acceptor::NewAccount(_) => None,
            };
            let held = hold_live_link(scope, token_hash, now, inviter.standing, replacing);
            let link = match held.await? {
                Ok(link) => link,
                Err(refused) => return Ok(refused),
            };
            let user_id = match acceptor {
                Acceptor::Account(user_id) => user_id.to_owned(),
                Acceptor::NewAccount(user) => {
                    accounts::insert_account(scope, user, InstanceRole::User).await?;
                    user.id.clone()
                }
                Acceptor::Replacing { user, .. } => {
                    if let Some((browser, turn)) = replacing {
                        signups::discard_of_browser(scope, browser, turn).await?;
                    }
                    accounts::insert_account(scope, user, InstanceRole::User).await?;
                    user.id.clone()
                }
            };
            let used = let_in(scope, &link, &user_id, inviter.of_container).await?;
            if used {
                take_use(scope, &link.id).await?;
            }
            Ok(Acceptance::Accepted(Accepted {
                user_id,
                workspace_id: link.workspace_id,
                container_id: link.container_id,
                used,
            }))
        })
        .await
    }
}

/// Holds the live link of the scope's organization whose token has the digest
/// `token_hash` at `now`, against the other acceptances of it, after what it hangs
/// from (`link_locks::hold_for_letting_in`), and its maker's
/// membership of the workspace if they are still `found`: the link, or what the
/// acceptance is to answer instead.
pub(crate) async fn hold_live_link(
    scope: &mut Scope<'_>,
    token_hash: &str,
    now: i64,
    found: Standing<'_>,
    replacing: Option<(&str, Turn)>,
) -> Result<Result<Invitation, Acceptance>, StoreError> {
    let (browser, turn) = (replacing.map(|(b, _)| b), replacing.map(|(_, t)| t));
    // What the link hangs from before the link, its invitation and membership: the
    // acceptor's role then stays what was read until its write.
    let sql = select_live_by_token(scope, token_hash, now, browser)?;
    let Some(link) = scope.first::<Invitation>(sql).await? else {
        return Ok(Err(Acceptance::Unavailable));
    };
    if !link_locks::hold_for_letting_in(scope, &link, turn).await? {
        return Ok(Err(Acceptance::Unavailable));
    }
    // The link itself, held against the other acceptances of it: read again,
    // it is live still, and its workspace still invites guests if it is for one.
    let mut sql = select_live_by_token(scope, token_hash, now, browser)?;
    push_invites(&mut sql);
    sql.push_lock(scope.row_lock(LockRank::Invitation, &link.id, RowLock::Update.of("i")));
    let Some(link) = scope.first::<Invitation>(sql).await? else {
        return Ok(Err(Acceptance::Unavailable));
    };
    if !inviter_still(scope, &link, found).await? {
        return Ok(Err(Acceptance::InviterChanged));
    }
    Ok(Ok(link))
}

/// Pushes the condition that the link `i` is for members, or that its workspace still
/// invites guests.
pub(crate) fn push_invites(sql: &mut Sql) {
    sql.push_bind(" AND (i.role <> ", WorkspaceRole::Guest.as_str())
        .push_bind(
            " OR EXISTS (SELECT 1 FROM workspaces AS t WHERE t.id = i.workspace_id \
             AND t.guest_invitations = ",
            true,
        )
        .push("))");
}

/// Lets the user `user_id` in by `link`, as [`Store::accept_invitation`] says, its
/// maker being `of_container` to a guest's board; says whether anything changed, which
/// counts as a use of the link.
pub(crate) async fn let_in(
    scope: &mut Scope<'_>,
    link: &Invitation,
    user_id: &str,
    of_container: InviterOfContainer,
) -> Result<bool, StoreError> {
    // What the link gives is said by its role: a row whose board does not go
    // with its role is broken, and lets nobody in as anything.
    match (link.role, &link.container_id, link.container_role) {
        (WorkspaceRole::Member, None, None) => {
            let_in_as_member(scope, &link.workspace_id, user_id).await
        }
        (WorkspaceRole::Guest, Some(container_id), Some(container_role)) => {
            let guest = ContainerMember::new(container_id, user_id, container_role);
            let let_in = let_in_as_guest(scope, &link.workspace_id, guest).await?;
            // Whom a manager who does not read the board puts on it is on the
            // record, by a link as by adding them.
            if let_in.onto_container && of_container == InviterOfContainer::ManagerOnly {
                let entry = AuditEntry::new(
                    &link.workspace_id,
                    &link.created_by,
                    AuditAction::AddedToContainer,
                    container_id,
                )
                .with_subject(user_id);
                audit::record_entry(scope, &entry).await?;
            }
            Ok(let_in.changed)
        }
        _ => Err(StoreError::Corrupt {
            column: "role",
            reason: "an invitation is for members, or for a guest of a board".to_owned(),
        }),
    }
}

/// Counts `uses` more uses of the link `link_id` of the scope's organization, or gives
/// some back where `uses` is negative (never below none).
pub(crate) async fn count_uses(
    scope: &mut Scope<'_>,
    link_id: &str,
    uses: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE invitations SET uses = uses + ");
    sql.bind(uses)
        .push_bind(" WHERE id = ", Id(link_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push_bind(" AND uses + ", uses)
        .push(" >= 0");
    scope.execute(sql).await?;
    Ok(())
}

/// Counts one more use of the held link `link_id` of the scope's organization, if it
/// may be used once more. A link is held live before this, which decides for every
/// link the server makes; this is what stands behind that where a link was found live
/// only by a registration that the acceptance replaces.
///
/// # Errors
///
/// Returns [`StoreError::Conflict`] with [`Taken::LinkUse`] if the link was used as
/// often as it may be.
pub(crate) async fn take_use(scope: &mut Scope<'_>, link_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE invitations SET uses = uses + 1 WHERE id = ");
    sql.bind(Id(link_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND (max_uses IS NULL OR uses < max_uses)");
    if scope.execute(sql).await? == 0 {
        return Err(StoreError::Conflict(Taken::LinkUse));
    }
    Ok(())
}

/// Makes the user a member of the scope's organization and of the workspace; says
/// whether anything changed. A guest of the workspace becomes a member of it.
async fn let_in_as_member(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let joined_org = accounts::join_organization(scope, user_id, OrgRole::Member).await?;
    let joined = accounts::join_workspace(
        scope,
        Which::Workspace(workspace_id),
        user_id,
        WorkspaceRole::Member,
    )
    .await?;
    if joined {
        // Who comes into the workspace anew starts anew: a role that was stored for
        // them on a board of it while they were being taken out is not theirs again.
        forget_container_roles(scope, workspace_id, user_id).await?;
    }
    let mut sql = scope.sql("UPDATE workspace_members SET role = ");
    sql.bind(WorkspaceRole::Member.as_str())
        .push_bind(" WHERE workspace_id = ", Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push_bind(" AND role = ", WorkspaceRole::Guest.as_str());
    let raised = scope.execute(sql).await? > 0;
    Ok(joined_org || joined || raised)
}

/// Whether the maker of the link is still what the server found them to be: the same
/// in the link's workspace, and the same on the board of a guest's link, which the
/// acceptance holds already. Read with the link held, and their membership of the
/// workspace is held from here on: who takes them out, or makes them something else
/// there, waits for the acceptance. The boards of the trash are held before the
/// membership, as whoever changes a membership has them.
pub(crate) async fn inviter_still(
    scope: &mut Scope<'_>,
    link: &Invitation,
    found: Standing<'_>,
) -> Result<bool, StoreError> {
    if found.user_id != link.created_by {
        return Ok(false);
    }
    trash::hold_deleted_containers(scope, &link.workspace_id).await?;
    let mut sql = scope.sql("SELECT role FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(&link.workspace_id))
        .push_bind(" AND user_id = ", Id(&link.created_by))
        .push_lock(scope.row_lock(
            LockRank::WorkspaceMembers,
            &format!("{}:{}", link.workspace_id, link.created_by),
            RowLock::Share,
        ));
    let role =
        match scope.fetch_optional(sql).await? {
            Some(row) => Some(row.text(0)?.parse::<WorkspaceRole>().map_err(|error| {
                StoreError::Corrupt {
                    column: "role",
                    reason: error.to_string(),
                }
            })?),
            None => None,
        };
    let member = match &link.container_id {
        Some(container_id) => find_member(scope, container_id, &link.created_by).await?,
        None => None,
    };
    let team_role = match &link.container_id {
        Some(container_id) => {
            crate::container_team_access::role_for(scope, container_id, &link.created_by).await?
        }
        None => None,
    };
    Ok(role == found.workspace_role
        && member.as_ref().map(|m| m.role) == found.member.map(|m| m.role)
        && team_role == found.team_role)
}

/// What letting a guest in changed.
struct GuestLetIn {
    /// Whether anything changed: the acceptance counts as a use of the link then.
    changed: bool,
    /// Whether the user was given a role on the board they did not have.
    onto_container: bool,
}

/// Makes the user of `invited` a member of its board, in its role or the higher one
/// they have there, and a guest of the workspace if they are no member of it; says
/// what changed.
async fn let_in_as_guest(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    invited: ContainerMember,
) -> Result<GuestLetIn, StoreError> {
    let user_id = &invited.user_id;
    // Who comes into the workspace as a guest is on the board of the link and on no
    // other: a role left on a board of it from an earlier time there opens nothing
    // while they are outside, and is not to open anything again by this.
    let mut sql = scope.sql("SELECT 1 FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    if !scope.exists(sql).await? {
        forget_container_roles(scope, workspace_id, user_id).await?;
    }
    let joined = accounts::join_workspace(
        scope,
        Which::Workspace(workspace_id),
        user_id,
        WorkspaceRole::Guest,
    )
    .await?;
    let held = find_member(scope, &invited.container_id, user_id)
        .await?
        .map(|member| member.role);
    if held >= Some(invited.role) {
        return Ok(GuestLetIn {
            changed: joined,
            onto_container: false,
        });
    }
    save_member(scope, &invited).await?;
    Ok(GuestLetIn {
        changed: true,
        onto_container: true,
    })
}

/// Takes from a user who is no member of the workspace the roles left on its boards,
/// and those its boards in the trash would come back with.
async fn forget_container_roles(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM container_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(
            " AND container_id IN (SELECT id FROM containers WHERE workspace_id = ",
            Id(workspace_id),
        )
        .push(")");
    scope.execute(sql).await?;
    trash::change_member_of_deleted_containers(scope, workspace_id, user_id, TrashedRoles::Forget)
        .await
}

/// Starts the query for the live links of the scope's organization at `now`, as
/// [`push_live_but_for`] counts them, of makers whose account is in use
/// ([`push_maker_in_use`]); conditions follow with `AND`.
fn select_live(scope: &Scope<'_>, now: i64, replacing: Option<&str>) -> Result<Sql, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        COLUMNS_OF_I,
        " FROM invitations AS i WHERE i.org_id = ",
    ]);
    sql.bind(scope.org()?);
    push_live_but_for(&mut sql, now, replacing);
    push_maker_in_use(&mut sql);
    Ok(sql)
}

/// Pushes the condition that the account of who made the link `i` is in use: a link
/// lets nobody in while its maker's account waits to be deleted, and does again once
/// the asking is taken back.
pub(crate) fn push_maker_in_use(sql: &mut Sql) {
    sql.push(
        " AND EXISTS (SELECT 1 FROM users AS maker WHERE maker.id = i.created_by \
         AND maker.deleted_at IS NULL)",
    );
}

/// The query for the links of the scope's organization that are listed at `now`: the
/// live ones, and a used one while a child of it nobody used is live, so that it can
/// be revoked with its children.
fn select_listed(scope: &Scope<'_>, now: i64) -> Result<Sql, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        COLUMNS_OF_I,
        " FROM invitations AS i WHERE i.org_id = ",
    ]);
    sql.bind(scope.org()?)
        .push_bind(" AND i.revoked_at = ", 0_i64)
        .push_bind(" AND i.expires_at > ", now)
        .push(" AND (i.max_uses IS NULL OR i.uses < i.max_uses OR EXISTS (SELECT 1 FROM invitations AS c WHERE c.parent_id = i.id AND c.org_id = i.org_id")
        .push_bind(" AND c.revoked_at = ", 0_i64)
        .push_bind(" AND c.expires_at > ", now)
        .push(" AND c.uses = 0))");
    Ok(sql)
}

/// The query for the live link of the scope's organization with the token's digest.
fn select_live_by_token(
    scope: &Scope<'_>,
    token_hash: &str,
    now: i64,
    replacing: Option<&str>,
) -> Result<Sql, StoreError> {
    let mut sql = select_live(scope, now, replacing)?;
    sql.push_bind(" AND i.token_hash = ", token_hash);
    Ok(sql)
}

/// Pushes the conditions that the link `i` is live at `now`, as [`push_live`] does,
/// for the browser whose marker has the digest `replacing`: the use a registration
/// made from that browser took of the link does not count against it, since what the
/// browser does next replaces that registration and gives the use back.
pub(crate) fn push_live_but_for(sql: &mut Sql, now: i64, replacing: Option<&str>) {
    sql.push_bind(" AND i.revoked_at = ", 0_i64)
        .push_bind(" AND i.expires_at > ", now)
        .push(" AND (i.max_uses IS NULL OR i.uses");
    if let Some(browser) = replacing {
        sql.push_bind(
            " - (SELECT count(*) FROM signups AS s WHERE s.invitation_id = i.id \
             AND s.invitation_org_id = i.org_id AND s.browser_hash = ",
            browser,
        )
        .push(")");
    }
    sql.push(" < i.max_uses)");
}

/// Pushes the conditions that the link `i` is live at `now`: not revoked, not ended,
/// and not used as often as it may be.
pub(crate) fn push_live(sql: &mut Sql, now: i64) {
    push_live_but_for(sql, now, None);
}

impl FromRow for Invitation {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let corrupt = |column: &'static str| {
            move |error: fvoci_model::UnknownRole| StoreError::Corrupt {
                column,
                reason: error.to_string(),
            }
        };
        let container_id = row.id(3)?;
        let container_role = row.text(4)?;
        let max_uses = row.int(8)?;
        Ok(Invitation {
            id: row.id(0)?,
            workspace_id: row.id(1)?,
            role: row.text(2)?.parse().map_err(corrupt("role"))?,
            container_id: (!container_id.is_empty()).then_some(container_id),
            container_role: if container_role.is_empty() {
                None
            } else {
                Some(container_role.parse().map_err(corrupt("container_role"))?)
            },
            created_by: row.id(5)?,
            created_at: row.int(6)?,
            expires_at: row.int(7)?,
            // No link may be used zero times: a column without a number is one without
            // such an end.
            max_uses: (max_uses > 0).then_some(max_uses),
            uses: row.int(9)?,
            parent_id: Some(row.id(10)?).filter(|id| !id.is_empty()),
            recipient: Some(row.text(11)?).filter(|recipient| !recipient.is_empty()),
            token: None,
        })
    }
}
