//! Erasing an account: what is the person's goes, and what they wrote stays under an
//! identifier that names nobody.
//!
//! An account is erased once its wait ran out, by the housekeeping, or at once on the
//! operator's word. The erasing is steps, each a transaction, and one that was cut off
//! is taken up where it stopped:
//!
//! 1. [`Store::begin_account_erasure`] decides, and marks the account
//!    (`erase_started_at`): from then on the asking is not taken back. It is the same
//!    question as at the asking (`account_deletion.rs`), under the same locks, asked
//!    again because who administers what may have changed in the wait: an account that
//!    would leave the instance, or an organization with other people in it, without an
//!    administrator is not erased. An organization that has nobody else is put out of
//!    use here, its provider removed and its links revoked, so that nobody joins what
//!    is going; whoever was coming in through one of them just then is waited for, and
//!    asked about, first.
//! 2. [`Store::erase_account_from_organization`], for each organization the account
//!    has a place in ([`Store::organizations_of_account`]), through that organization's
//!    handle: the organization goes with everything in it if it was put out of use for
//!    this account, its only person (asked once more: somebody who came in past step 1
//!    keeps it); otherwise the account leaves each of its workspaces, in use or in the
//!    trash, as a member taken out of one does, and the organization, and the links it
//!    made there that nobody used go.
//! 3. [`Store::finish_account_erasure`] removes what is the person's of no organization
//!    (their identities, sessions, settings and credentials, and the names of their
//!    tokens wherever they were copied), takes who they are out of the audit log, and
//!    leaves of the row its identifier.
//!
//! Steps 1 and 3 reach every organization: a person's memberships, and the records
//! about them, are of several.

use fvoci_model::{
    AuditAction, ContainerMember, ContainerRole, OrgRole, OrganizationName, WorkspaceRole,
};

use crate::account_deletion::{self, DeletionStanding, Held};
use crate::audit::{self, AuditActor, AuditEntry};
use crate::containers::{delete_member, find_member, save_member};
use crate::db::{DeletedContainersHeld, Id, LockRank, OrganizationHeld, Param, RowLock, Scope};
use crate::error::StoreError;
use crate::sessions::{self, EndedSessionScope, SessionsEnded};
use crate::trash::{self, Purged, TrashedRoles};
use crate::{
    Store, email_revert, link_locks, members, org_admin, password_reset, users,
    workspace_membership,
};

/// On whose word an account is erased.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Erasing {
    /// Its own: it asked, and its wait ran out.
    AfterTheWait,
    /// The operator's, at once, whether it asked or not.
    AtOnce,
}

/// What became of beginning to erase an account.
#[derive(Debug, PartialEq, Eq)]
pub enum ErasureStart {
    /// It is marked, now or before: the steps after this are to be done, and any
    /// sessions ended in this call are carried in the witness.
    Begun(SessionsEnded),
    /// There is no such account, or it is erased.
    NotThere,
    /// Its wait has not run out, or it did not ask.
    Waits,
    /// Something would be left without anybody to run it: nothing changed.
    Refused(DeletionStanding),
}

/// A board or wiki that passed to another administrator of its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandedOver {
    /// The workspace of the board.
    pub workspace_id: String,
    /// The board.
    pub container_id: String,
    /// Who is its admin now.
    pub heir_id: String,
}

/// What went with an organization that went with the account of its only person: its
/// boards, blocks and files, and the sessions made through its provider.
#[must_use = "close the connections of the sessions it ended with Hub::close"]
#[derive(Debug, PartialEq, Eq)]
pub struct RemovedOrganization {
    /// Its boards and blocks, and their files.
    pub purged: Purged,
    /// The sessions made through its provider, which went with it.
    pub ended: SessionsEnded,
}

/// What an account's leaving an organization came to: as it is erased, or of its own
/// accord ([`Store::leave_organization`]), which removes nothing and keeps nothing for a
/// newcomer.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct LeftFrom {
    /// The workspaces the account left, in use or in the trash.
    pub left: Vec<String>,
    /// The boards that passed to somebody else, for the account was their only admin.
    pub handed_over: Vec<HandedOver>,
    /// What went with the organization, if it went: `None` for one that stays.
    pub removed: Option<RemovedOrganization>,
    /// Whether the organization was to go and stays, in use again, because somebody
    /// came into it after it was put out of use: it has no administrator then, and
    /// neither the provider's settings nor the links it had (they went when it was put
    /// out of use), which the server's operator is to be told.
    pub kept_for_newcomer: bool,
    /// The workspaces in use the account was the only administrator of: an
    /// administrator of the organization is to step in.
    pub workspaces_without_administrator: Vec<String>,
    /// The boards the account was the only admin of that passed to nobody, for their
    /// workspace has no other administrator on their own account.
    pub boards_without_admin: Vec<String>,
}

/// What became of ending the erasing of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErasureEnd {
    /// It is erased now.
    Erased,
    /// It was erased before: another run ended it.
    ErasedBefore,
    /// It still has a place in an organization: the steps before are to be done.
    StillPlaced,
    /// Its erasing did not begin, or there is no such account.
    NotBegun,
}

impl Store {
    /// The accounts to erase at `now`: those whose wait ran out, and those whose
    /// erasing began and did not end. The longest overdue first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn accounts_to_erase(&self, now: i64) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT id FROM users WHERE deleted_at IS NOT NULL AND erased_at = 0 \
                 AND (erase_started_at > 0 OR (delete_after > 0 AND delete_after <= ",
            );
            sql.bind(now).push(")) ORDER BY delete_after, id");
            scope.all(sql).await
        })
        .await
    }

    /// Marks the account as being erased, from `now`, if it is to be: its wait ran out
    /// ([`Erasing::AfterTheWait`]) or the operator says so ([`Erasing::AtOnce`]), and
    /// nothing would be left without anybody to run it. With the mark, in the same
    /// transaction: the account is out of use and its sessions are ended, if it was not
    /// and they were not; and each organization that has nobody else is put out of
    /// use, its provider removed and its links revoked, so that nobody comes into it.
    /// Somebody who was coming into one just then, by a link or through its provider,
    /// is waited for: with them in it the organization has other people, and its only
    /// administrator is not erased ([`ErasureStart::Refused`]).
    ///
    /// An asking that is taken back while this decides is taken back: the mark is set
    /// on an account that still is to be erased, or not at all
    /// ([`ErasureStart::Waits`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is changed then.
    pub async fn begin_account_erasure(
        &self,
        user_id: &str,
        now: i64,
        erasing: Erasing,
    ) -> Result<ErasureStart, StoreError> {
        self.system()
            .transaction(async |scope| {
                let held_instance = users::lock_instance(scope).await?;
                let mut sql = scope.sql(
                    "SELECT delete_after, erase_started_at FROM users \
                     WHERE erased_at = 0 AND id = ",
                );
                sql.bind(Id(user_id));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(ErasureStart::NotThere);
                };
                let (delete_after, started_at) = (row.int(0)?, row.int(1)?);
                if started_at > 0 {
                    return Ok(ErasureStart::Begun(SessionsEnded::none()));
                }
                let due = delete_after > 0 && delete_after <= now;
                if erasing == Erasing::AfterTheWait && !due {
                    return Ok(ErasureStart::Waits);
                }
                let mut standing =
                    account_deletion::standing(scope, user_id, Held::Yes(held_instance)).await?;
                // Whoever is on their way into an organization that is to go is in it
                // before it is asked once more who is.
                let (alone_in, joined) = {
                    let waited = std::mem::take(&mut standing.alone_in);
                    wait_for_who_comes_in(scope, &waited).await?;
                    let mut alone_in = Vec::new();
                    let mut joined = Vec::new();
                    for organization in waited {
                        if account_deletion::has_other_people(scope, &organization.id, user_id)
                            .await?
                        {
                            joined.push(organization);
                        } else {
                            alone_in.push(organization);
                        }
                    }
                    (alone_in, joined)
                };
                standing.alone_in = alone_in;
                standing.sole_admin_of.extend(joined);
                if !standing.is_open() {
                    return Ok(ErasureStart::Refused(standing));
                }
                let Some(ended) = mark(scope, user_id, now, erasing).await? else {
                    return Ok(ErasureStart::Waits);
                };
                // The account's own space goes with it whoever is in it.
                for organization in standing.alone_in.iter().chain(&standing.own_space) {
                    put_out_of_use(scope, &organization.id, now).await?;
                }
                Ok(ErasureStart::Begun(ended))
            })
            .await
    }

    /// The organizations an account has something in, by identifier: those it is a
    /// member of, those it is in a workspace or on a board of, those it made a link
    /// into, those it made, and its own space, whoever is a member of that.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn organizations_of_account(&self, user_id: &str) -> Result<Vec<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql("SELECT org_id FROM organization_members WHERE user_id = ");
                sql.bind(Id(user_id))
                    .push(
                        " UNION SELECT w.org_id FROM workspace_members AS wm \
                         JOIN workspaces AS w ON w.id = wm.workspace_id WHERE wm.user_id = ",
                    )
                    .bind(Id(user_id))
                    .push(&scope.same_organization(&[("wm", "w")]))
                    .push(
                        " UNION SELECT w.org_id FROM container_members AS cm \
                         JOIN containers AS c ON c.id = cm.container_id \
                         JOIN workspaces AS w ON w.id = c.workspace_id WHERE cm.user_id = ",
                    )
                    .bind(Id(user_id))
                    .push(&scope.same_organization(&[("cm", "c"), ("c", "w")]))
                    .push_bind(
                        " UNION SELECT org_id FROM invitations WHERE created_by = ",
                        Id(user_id),
                    )
                    .push_bind(
                        " UNION SELECT id FROM organizations WHERE owner_id = ",
                        Id(user_id),
                    )
                    // One it made and has no place in any more: it names the account.
                    .push_bind(
                        " UNION SELECT id FROM organizations WHERE created_by = ",
                        Id(user_id),
                    )
                    .push(" ORDER BY 1");
                scope.all(sql).await
            })
            .await
    }

    /// Takes an account whose erasing began out of the handle's organization: the
    /// server's own doing where its wait ran out, its operator's where they said so,
    /// and recorded as that.
    ///
    /// The account's own space goes with everything in it, in use or not and whoever
    /// is in it. An organization that was put out of use for this account, its only
    /// person and its administrator, and still has nobody else, goes the same way, as
    /// a workspace removed for good goes ([`Store::purge_workspace`]): its files are
    /// doomed. One put out of use for another account, in which this one has something
    /// left, is that account's to take along or to keep. Otherwise the account leaves
    /// each workspace of the organization, in use or in the trash, with its roles on
    /// the boards there, its places in the sidebar and what it follows; a
    /// board it was the only admin of passes to the oldest other administrator of the
    /// workspace who is in it on their own account (not as the organization's), and
    /// that is recorded; with no such administrator the board is left for whoever
    /// administers the workspace next, and a workspace it alone administered for an
    /// administrator of the organization, both told to the caller. Then it leaves the
    /// organization, which names it no more as who made it, and the links it made
    /// there that nobody used, and that no other link was made like, go. What it wrote
    /// stays.
    ///
    /// Nothing happens for an account whose erasing did not begin.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails; nothing is changed then.
    pub async fn erase_account_from_organization(
        &self,
        user_id: &str,
        erasing: Erasing,
    ) -> Result<LeftFrom, StoreError> {
        let actor = &match erasing {
            Erasing::AfterTheWait => AuditActor::Server,
            Erasing::AtOnce => AuditActor::ServerOperator,
        };
        self.transaction(async |scope| {
            let held_org = org_admin::hold_organization_row(scope).await?;
            if !is_being_erased(scope, user_id).await? {
                return Ok(LeftFrom::default());
            }
            let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
            let mut erased = LeftFrom::default();
            if is_own_space_of(scope, user_id).await? {
                erased.removed = Some(remove_organization(scope, held_org).await?);
                return Ok(erased);
            }
            if is_out_of_use(scope).await? && administers_organization(scope, user_id).await? {
                if !account_deletion::has_other_people(scope, &org_id, user_id).await? {
                    erased.removed = Some(remove_organization(scope, held_org).await?);
                    return Ok(erased);
                }
                // Somebody came in between the question and the mark: the
                // organization is theirs to keep.
                put_back_in_use(scope).await?;
                erased.kept_for_newcomer = true;
            }
            leave_every_workspace(scope, user_id, actor, &mut erased).await?;
            leave_organization(scope, held_org, user_id).await?;
            // Who made the organization is nobody's to know once the account is gone: an
            // erased account makes none, and is counted none. One that leaves of its own
            // accord is counted still.
            let mut sql =
                scope.sql("UPDATE organizations SET created_by = NULL WHERE created_by = ");
            sql.bind(Id(user_id)).push_bind(" AND id = ", Id(&org_id));
            scope.execute(sql).await?;
            Ok(erased)
        })
        .await
    }

    /// Ends the erasing of an account at `now`: removes its identities, its sessions,
    /// its settings and credentials, the names of its tokens from what they were
    /// copied into (the versions and documents an agent wrote, and what others were
    /// told the account did through one), the links mailed to its own
    /// address that nobody used (and the address from those that were), and who it is
    /// from the records of the audit log about it, in every organization; and leaves of
    /// its row the identifier, with no name, address or password. Not while the account
    /// still has a place in an organization, and not for an account whose erasing did
    /// not begin or ended before: the answer says which.
    ///
    /// The links are of any organization: they are held as `link_locks` has it, under
    /// the turn and each organization's row, before the account's own row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is changed then.
    pub async fn finish_account_erasure(
        &self,
        user_id: &str,
        now: i64,
    ) -> Result<ErasureEnd, StoreError> {
        self.system()
            .transaction(async |scope| {
                let turn = link_locks::take_turn(scope).await?;
                let addresses = addresses_of(scope, user_id).await?;
                hold_organizations_of_links_to(scope, turn, &addresses).await?;
                let mut sql =
                    scope.sql("SELECT erase_started_at, erased_at FROM users WHERE id = ");
                sql.bind(Id(user_id));
                sql.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::Update));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(ErasureEnd::NotBegun);
                };
                if row.int(1)? > 0 {
                    return Ok(ErasureEnd::ErasedBefore);
                }
                if row.int(0)? == 0 {
                    return Ok(ErasureEnd::NotBegun);
                }
                if has_a_place(scope, user_id).await? {
                    return Ok(ErasureEnd::StillPlaced);
                }
                forget_addresses(scope, &addresses).await?;
                // A trial of pending provider settings it started and did not end.
                let mut sql = scope.sql("DELETE FROM oidc_login_flows WHERE pending_by = ");
                sql.bind(Id(user_id));
                scope.execute(sql).await?;
                let mut sql = scope.sql(
                    "UPDATE audit_log SET identity = NULL WHERE identity IS NOT NULL \
                     AND (actor_id = ",
                );
                sql.bind(Id(user_id))
                    .push_bind(" OR subject_id = ", Id(user_id))
                    .push(")");
                scope.execute(sql).await?;
                crate::api_tokens::forget_names_of_tokens_of(scope, user_id).await?;
                crate::card_events::forget_actor(scope, user_id).await?;
                for delete in PERSONAL_DELETES {
                    let mut sql = scope.sql(delete);
                    sql.bind(Id(user_id));
                    scope.execute(sql).await?;
                }
                let mut sql = scope.sql(
                    "UPDATE users SET username = '', email = '', email_verified_at = 0, \
                     display_name = '', custom_name = '', first_name = '', last_name = '', \
                     latin_first_name = '', latin_last_name = '', title = '', \
                     credentials = '', name_shown = 'name', password_hash = '', \
                     delete_after = 0, is_admin = ",
                );
                sql.bind(false)
                    .push_bind(", erased_at = ", now)
                    .push_bind(", updated_at = ", now)
                    .push_bind(" WHERE id = ", Id(user_id));
                scope.execute(sql).await?;
                Ok(ErasureEnd::Erased)
            })
            .await
    }
}

/// The tables of what is a person's alone, and the column that names them: what goes
/// when their account is erased, whichever organization a row is of. An upload that
/// was open is not among them: nobody sends its parts any more, and its clean-up needs
/// the row to throw away what was sent (`uploads.rs`).
///
/// `meeting_attendance` and `course_meetings` (`presenter_id`, `host_id`) are intentionally
/// excluded: shared study meeting history stays under the erased user's tombstone identifier.
const PERSONAL_DELETES: [&str; 22] = [
    "DELETE FROM notifications WHERE recipient_id = ",
    "DELETE FROM notification_outbox WHERE recipient_id = ",
    "DELETE FROM card_due_notices WHERE user_id = ",
    "DELETE FROM card_assignees WHERE user_id = ",
    "DELETE FROM time_entries WHERE user_id = ",
    "DELETE FROM user_identities WHERE user_id = ",
    "DELETE FROM email_changes WHERE user_id = ",
    "DELETE FROM email_reverts WHERE user_id = ",
    "DELETE FROM sessions WHERE user_id = ",
    "DELETE FROM api_tokens WHERE user_id = ",
    "DELETE FROM user_settings WHERE user_id = ",
    "DELETE FROM user_client_state WHERE user_id = ",
    "DELETE FROM password_resets WHERE user_id = ",
    "DELETE FROM subscriptions WHERE user_id = ",
    "DELETE FROM reactions WHERE user_id = ",
    "DELETE FROM category_containers WHERE user_id = ",
    "DELETE FROM categories WHERE user_id = ",
    "DELETE FROM favorites WHERE user_id = ",
    "DELETE FROM unit_progress WHERE user_id = ",
    "DELETE FROM exam_scores WHERE user_id = ",
    "DELETE FROM unit_reviews WHERE user_id = ",
    "DELETE FROM unit_notes WHERE user_id = ",
];

/// Marks the account as being erased from `now`, puts it out of use if it was in use,
/// and ends its sessions and credentials. Returns the sessions it ended, or `None`
/// where it marked nothing: an account that is no more to be erased, its asking taken
/// back since it was read, which the statement waits for and sees.
async fn mark(
    scope: &mut Scope<'_>,
    user_id: &str,
    now: i64,
    erasing: Erasing,
) -> Result<Option<SessionsEnded>, StoreError> {
    let mut sql = scope.sql("UPDATE users SET erase_started_at = ");
    sql.bind(now)
        .push_bind(", deleted_at = COALESCE(deleted_at, ", now)
        .push_bind("), updated_at = ", now)
        .push_bind(
            " WHERE erase_started_at = 0 AND erased_at = 0 AND id = ",
            Id(user_id),
        );
    if erasing == Erasing::AfterTheWait {
        sql.push_bind(" AND delete_after > 0 AND delete_after <= ", now);
    }
    if scope.execute(sql).await? == 0 {
        return Ok(None);
    }
    let ended = sessions::delete_sessions_of_user(scope, user_id).await?;
    password_reset::end_links_of_user(scope, user_id).await?;
    email_revert::end_of_user(scope, user_id).await?;
    Ok(Some(ended))
}

/// Waits for whoever is on their way into the `organizations`, on PostgreSQL: a sign-in
/// through a provider holds the provider's settings, and an acceptance its link, until
/// it ends. Held here in turn, after the organizations' rows (`link_locks`), they are in
/// an organization, or refused, before it is asked who is in it; whoever comes after
/// waits for this transaction, and finds the provider gone and the link revoked. The
/// providers of every organization are held before the links of any, in the order of
/// the organizations, as their ranks have them. SQLite's writers wait for each other
/// already.
async fn wait_for_who_comes_in(
    scope: &mut Scope<'_>,
    organizations: &[OrganizationName],
) -> Result<(), StoreError> {
    for (rank, select) in [
        (
            LockRank::Provider,
            "SELECT id FROM oidc_providers WHERE org_id = ",
        ),
        (
            LockRank::Invitation,
            "SELECT id FROM invitations WHERE org_id = ",
        ),
    ] {
        for organization in organizations {
            let mut sql = scope.sql(select);
            sql.bind(Id(&organization.id)).push(" ORDER BY id");
            scope
                .hold(rank, &organization.id, sql, RowLock::Update)
                .await?;
        }
    }
    Ok(())
}

/// Puts an organization out of use at `now` and closes the ways into it: its provider
/// is removed, with the sign-ins under way through it, and its links are revoked.
async fn put_out_of_use(scope: &mut Scope<'_>, org_id: &str, now: i64) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE organizations SET deleted_at = ");
    sql.bind(now)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE deleted_at IS NULL AND id = ", Id(org_id));
    scope.execute(sql).await?;
    let mut sql = scope.sql("DELETE FROM oidc_providers WHERE org_id = ");
    sql.bind(Id(org_id));
    scope.execute(sql).await?;
    let mut sql = scope.sql("UPDATE invitations SET revoked_at = ");
    sql.bind(now)
        .push_bind(" WHERE revoked_at = 0 AND org_id = ", Id(org_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Puts the scope's organization, which was put out of use to go, back in use.
async fn put_back_in_use(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE organizations SET deleted_at = NULL WHERE id = ");
    sql.bind(scope.org()?);
    scope.execute(sql).await?;
    Ok(())
}

/// Whether the erasing of the account began and did not end.
async fn is_being_erased(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql =
        scope.sql("SELECT 1 FROM users WHERE erase_started_at > 0 AND erased_at = 0 AND id = ");
    sql.bind(Id(user_id));
    scope.exists(sql).await
}

/// Whether the scope's organization is the user's own space.
async fn is_own_space_of(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organizations WHERE id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND owner_id = ", Id(user_id));
    scope.exists(sql).await
}

/// Whether the scope's organization is out of use.
async fn is_out_of_use(scope: &mut Scope<'_>) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organizations WHERE deleted_at IS NOT NULL AND id = ");
    sql.bind(scope.org()?);
    scope.exists(sql).await
}

/// Whether the user is an administrator of the scope's organization: an organization
/// put out of use to go was put so for its administrator, and for nobody else who has
/// something left in it.
async fn administers_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organization_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push_bind(" AND role = ", OrgRole::Admin.as_str());
    scope.exists(sql).await
}

/// Takes the user out of each workspace of the scope's organization they are in, in use
/// or in the trash ([`leave_workspace`]), and writes down what came of it in `left`.
pub(crate) async fn leave_every_workspace(
    scope: &mut Scope<'_>,
    user_id: &str,
    actor: &AuditActor,
    left: &mut LeftFrom,
) -> Result<(), StoreError> {
    for workspace_id in workspaces_of(scope, user_id).await? {
        let Some(out) = leave_workspace(scope, &workspace_id, user_id, actor).await? else {
            continue;
        };
        left.handed_over.extend(out.handed_over);
        left.boards_without_admin.extend(out.without_admin);
        if out.without_administrator {
            left.workspaces_without_administrator
                .push(workspace_id.clone());
        }
        left.left.push(workspace_id);
    }
    Ok(())
}

/// The workspaces of the scope's organization the user is in, in use or in the trash,
/// by identifier.
async fn workspaces_of(scope: &mut Scope<'_>, user_id: &str) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql(
        "SELECT wm.workspace_id FROM workspace_members AS wm \
         JOIN workspaces AS w ON w.id = wm.workspace_id WHERE wm.user_id = ",
    );
    sql.bind(Id(user_id))
        .push_bind(" AND w.org_id = ", scope.org()?)
        .push(" ORDER BY wm.workspace_id");
    scope.all(sql).await
}

/// Takes the user out of a workspace of the scope's organization, in use or in the
/// trash, as `workspace_membership` takes a member out, but for two things: the last
/// administrator goes too (an administrator of the organization steps in), and a board
/// left without an admin passes to another administrator of the workspace, not to who
/// asks. The locks are taken in the same order. `None` for a workspace that is not
/// there.
async fn leave_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    actor: &AuditActor,
) -> Result<Option<LeftWorkspace>, StoreError> {
    let mut sql = scope.sql("SELECT deleted_at FROM workspaces WHERE id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND org_id = ", scope.org()?);
    sql.push_lock(scope.row_lock(LockRank::Workspace, workspace_id, RowLock::NoKeyUpdate));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    let in_use = row.opt_int(0)?.is_none();
    let containers = workspace_membership::hold_containers_of(scope, workspace_id, user_id).await?;
    let held_trashed = trash::hold_deleted_containers(scope, workspace_id).await?;
    let role = workspace_membership::held_role(scope, workspace_id, user_id).await?;
    let mut left = LeftWorkspace {
        without_administrator: in_use
            && role == Some(WorkspaceRole::Admin)
            && workspace_membership::is_only_administrator(scope, workspace_id, user_id).await?,
        ..LeftWorkspace::default()
    };
    let heir = heir_in(scope, workspace_id, user_id).await?;
    for container_id in &containers {
        let administers = find_member(scope, container_id, user_id)
            .await?
            .is_some_and(|member| member.role == ContainerRole::Admin);
        let alone = administers && members::is_only_admin(scope, container_id, user_id).await?;
        if alone && heir.is_none() {
            left.without_admin.push(container_id.clone());
        }
        if alone && let Some(heir_id) = &heir {
            let admin = ContainerMember::new(container_id, heir_id, ContainerRole::Admin);
            save_member(scope, &admin).await?;
            let entry = AuditEntry::by(
                actor,
                workspace_id,
                AuditAction::AddedToContainer,
                container_id,
            )
            .with_subject(heir_id);
            audit::record_entry(scope, &entry).await?;
            left.handed_over.push(HandedOver {
                workspace_id: workspace_id.to_owned(),
                container_id: container_id.clone(),
                heir_id: heir_id.clone(),
            });
        }
        delete_member(scope, container_id, user_id).await?;
    }
    trash::change_member_of_deleted_containers(scope, workspace_id, user_id, TrashedRoles::Forget)
        .await?;
    forget_places_in_the_trash(scope, held_trashed, workspace_id, user_id).await?;
    crate::unit_progress::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::course_exams::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::unit_reviews::forget_user_in_workspace(scope, workspace_id, user_id).await?;
    crate::container_team_membership::remove_workspace_teams(scope, workspace_id, user_id).await?;
    crate::api_tokens::forget_workspace_of_user(scope, workspace_id, user_id).await?;
    let mut sql = scope.sql("DELETE FROM workspace_members WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND user_id = ", Id(user_id));
    scope.execute(sql).await?;
    Ok(Some(left))
}

/// What an account's leaving a workspace came to.
#[derive(Debug, Default)]
struct LeftWorkspace {
    /// The boards that passed to the heir.
    handed_over: Vec<HandedOver>,
    /// The boards the account alone was the admin of, when there is no heir.
    without_admin: Vec<String>,
    /// Whether the workspace is in use and the account alone administered it.
    without_administrator: bool,
}

/// Takes the places the user filed the boards of a workspace under out of what its
/// boards in the trash would come back with (`deleted_containers.places`): the places
/// are theirs alone, and go with them. The caller holds the boards in the trash.
async fn forget_places_in_the_trash<'c>(
    scope: &mut Scope<'c>,
    _held: DeletedContainersHeld<'c>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id, places FROM deleted_containers WHERE workspace_id = ");
    sql.bind(Id(workspace_id));
    for row in scope.fetch_all(sql).await? {
        let places: Vec<serde_json::Value> = trash::from_json(&row, 1, "places")?;
        let theirs = |place: &serde_json::Value| place["userId"] == user_id;
        if !places.iter().any(theirs) {
            continue;
        }
        let kept: Vec<serde_json::Value> =
            places.into_iter().filter(|place| !theirs(place)).collect();
        let mut sql = scope.sql("UPDATE deleted_containers SET places = ");
        sql.bind(Param::json(&kept)?)
            .push_bind(" WHERE id = ", Id(&row.id(0)?));
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Who a board of the workspace passes to when the user goes: the administrator of the
/// workspace, other than the user, who has been one longest among those whose account
/// is in use and who are in the workspace on their own account. One who joined as the
/// organization's administrator is none: what they got while joined ends with the
/// joining.
async fn heir_in(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<String>, StoreError> {
    let mut sql = scope.sql(
        "SELECT wm.user_id FROM workspace_members AS wm JOIN users AS u ON u.id = wm.user_id \
         WHERE u.deleted_at IS NULL AND wm.workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(" AND wm.user_id <> ", Id(user_id))
        .push_bind(" AND wm.role = ", WorkspaceRole::Admin.as_str())
        .push_bind(" AND wm.via_org_admin = ", false)
        .push(" ORDER BY wm.created_at, wm.user_id LIMIT 1");
    scope.first(sql).await
}

/// Removes what the user still has in the scope's organization once they are out of
/// its workspaces: a role left on a board of a workspace they had left before, what
/// they follow there, their reactions there, the membership of the organization, and the links they made
/// that nobody used. A link another was made like stays, and so do those made like it
/// that were used and the registrations made by them, which the schema would take
/// along: such a link lets nobody in, for its maker is no member of the organization.
/// The caller holds the organization's row, as whoever holds several of its links does
/// (`link_locks`).
pub(crate) async fn leave_organization<'c>(
    scope: &mut Scope<'c>,
    _held: OrganizationHeld<'c>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM container_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push(
            " AND container_id IN (SELECT c.id FROM containers AS c \
             JOIN workspaces AS w ON w.id = c.workspace_id WHERE w.org_id = ",
        )
        .bind(scope.org()?)
        .push(")");
    scope.execute(sql).await?;
    // Also what they followed without a role: a page of a wiki open to its workspace.
    let mut sql = scope.sql("DELETE FROM subscriptions WHERE user_id = ");
    sql.bind(Id(user_id))
        .push(
            " AND block_id IN (SELECT b.id FROM blocks AS b \
             JOIN containers AS c ON c.id = b.container_id \
             JOIN workspaces AS w ON w.id = c.workspace_id WHERE w.org_id = ",
        )
        .bind(scope.org()?)
        .push(")");
    scope.execute(sql).await?;
    trash::forget_watcher_of_organization(scope, user_id).await?;
    crate::unit_progress::forget_user_in_organization(scope, user_id).await?;
    crate::course_exams::forget_user_in_organization(scope, user_id).await?;
    // Serialize with concurrent `start_time_entry` / `create_time_entry` / personal review
    // and note writes (which hold `users`) so an in-flight write cannot insert an orphan
    // after we clear rows.
    crate::users::hold_user_for_cleanup(scope, user_id).await?;
    crate::reactions::forget_user_in_organization(scope, user_id).await?;
    crate::unit_reviews::forget_user_in_organization(scope, user_id).await?;
    let mut due_sql = scope.sql("DELETE FROM card_due_notices WHERE user_id = ");
    due_sql
        .bind(Id(user_id))
        .push_bind(" AND org_id = ", scope.org()?);
    scope.execute(due_sql).await?;
    let mut sql = scope.sql("DELETE FROM time_entries WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(" AND org_id = ", scope.org()?);
    scope.execute(sql).await?;
    crate::api_tokens::forget_organization_of_user(scope, user_id).await?;
    let mut sql = scope.sql("DELETE FROM organization_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(" AND org_id = ", scope.org()?);
    scope.execute(sql).await?;
    // Twice: the links made like another for the recipients of its mail first, and then
    // the link they were made like, once none is left of them.
    for _ in 0..2 {
        let mut sql = scope.sql("DELETE FROM invitations WHERE uses = 0 AND created_by = ");
        sql.bind(Id(user_id))
            .push_bind(" AND org_id = ", scope.org()?)
            .push(UNLIKE_ANY);
        scope.execute(sql).await?;
    }
    Ok(())
}

/// No link was made like the one of the statement.
const UNLIKE_ANY: &str = " AND NOT EXISTS (SELECT 1 FROM invitations AS made_like \
                          WHERE made_like.parent_id = invitations.id)";

/// Removes the scope's organization and everything in it: each of its workspaces as a
/// workspace goes for good (`org_admin::remove_workspace`), the sessions made through
/// its provider, which it names, and with its row what hangs from it (its members, its
/// links, its provider settings and its audit log: the schema lets a record go once
/// its organization is not there). What the search server and the file storage are
/// still to be told (`search_outbox`, `loose_objects`) stays until they were.
async fn remove_organization<'c>(
    scope: &mut Scope<'c>,
    held_org: OrganizationHeld<'c>,
) -> Result<RemovedOrganization, StoreError> {
    let mut sql = scope.sql("UPDATE organizations SET default_workspace_id = NULL WHERE id = ");
    sql.bind(scope.org()?);
    scope.execute(sql).await?;
    // Under the organization's row, which is held: each of them is a pass of its own
    // after this.
    let org_id = scope.org_id().unwrap_or_default();
    let mut sql = scope.sql("SELECT id FROM workspaces WHERE org_id = ");
    sql.bind(scope.org()?)
        .push(" ORDER BY id")
        .push_lock(scope.row_lock(LockRank::Organization, &org_id, RowLock::Update));
    let workspaces: Vec<String> = scope.all(sql).await?;
    let mut removed = Purged::default();
    for workspace_id in &workspaces {
        scope.record_lock(LockRank::Workspace, workspace_id, RowLock::Update);
        let of_workspace = org_admin::remove_workspace(scope, held_org, workspace_id).await?;
        removed.records += of_workspace.records;
        removed.files.extend(of_workspace.files);
    }
    let mut sql = scope.sql("DELETE FROM time_entries WHERE org_id = ");
    sql.bind(scope.org()?);
    scope.execute(sql).await?;
    crate::api_tokens::revoke_of_organization_providers(scope).await?;
    let mut sql = scope.sql("SELECT id FROM sessions WHERE login_org_id = ");
    sql.bind(scope.org()?);
    let sessions: Vec<String> = scope.all(sql).await?;
    let mut sql = scope.sql("DELETE FROM sessions WHERE login_org_id = ");
    sql.bind(scope.org()?);
    scope.execute(sql).await?;
    let mut sql = scope.sql("DELETE FROM organizations WHERE id = ");
    sql.bind(scope.org()?);
    scope.execute(sql).await?;
    Ok(RemovedOrganization {
        purged: removed,
        ended: if sessions.is_empty() {
            SessionsEnded::none()
        } else {
            SessionsEnded::new(EndedSessionScope::Sessions(sessions))
        },
    })
}

/// Whether the account still has a place in an organization, whichever.
async fn has_a_place(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    for select in [
        "SELECT 1 FROM organization_members WHERE user_id = ",
        "SELECT 1 FROM workspace_members WHERE user_id = ",
        "SELECT 1 FROM container_members WHERE user_id = ",
    ] {
        let mut sql = scope.sql(select);
        sql.bind(Id(user_id)).push(" LIMIT 1");
        if scope.exists(sql).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The address of the account that is its own (`users.email`), as a list: none for an
/// account that is not there, or has none left. What a provider gave for an identity of
/// the account is the provider's word, checked by nobody: it may be somebody else's
/// address, and nothing mailed to it is touched. The identities go with their rows.
async fn addresses_of(scope: &mut Scope<'_>, user_id: &str) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT email FROM users WHERE id = ");
    sql.bind(Id(user_id));
    let mut addresses = Vec::new();
    for row in scope.fetch_all(sql).await? {
        let address = row.text(0)?;
        if !address.is_empty() {
            addresses.push(address);
        }
    }
    Ok(addresses)
}

/// Holds the row of each organization that has a link mailed to one of the addresses,
/// in the order of their identifiers: the caller holds the turn, and holds the links
/// after this.
async fn hold_organizations_of_links_to(
    scope: &mut Scope<'_>,
    _turn: link_locks::Turn,
    addresses: &[String],
) -> Result<(), StoreError> {
    if addresses.is_empty() {
        return Ok(());
    }
    // The letters are folded by the database on both sides, as it folds them wherever
    // an address is compared.
    let mut sql = scope.sql("SELECT DISTINCT org_id FROM invitations WHERE ");
    for (index, address) in addresses.iter().enumerate() {
        let or = if index == 0 { "" } else { " OR " };
        sql.push(or)
            .push_bind("lower(recipient) = lower(", address.as_str())
            .push(")");
    }
    sql.push(" ORDER BY org_id");
    let organizations: Vec<String> = scope.all(sql).await?;
    for org_id in &organizations {
        link_locks::hold_organization(scope, org_id).await?;
    }
    Ok(())
}

/// Removes the links that were mailed to one of the addresses and that nobody used,
/// takes the address out of those that were used, and removes the sign-ins through a
/// provider that are held for somebody of the address to say that the account is
/// theirs. The caller holds the organizations of the links.
async fn forget_addresses(scope: &mut Scope<'_>, addresses: &[String]) -> Result<(), StoreError> {
    for address in addresses {
        let mut sql =
            scope.sql("DELETE FROM invitations WHERE uses = 0 AND lower(recipient) = lower(");
        sql.bind(address.as_str()).push(")").push(UNLIKE_ANY);
        scope.execute(sql).await?;
        let mut sql =
            scope.sql("UPDATE invitations SET recipient = NULL WHERE lower(recipient) = lower(");
        sql.bind(address.as_str()).push(")");
        scope.execute(sql).await?;
        let mut sql = scope.sql("DELETE FROM oidc_held_sign_ins WHERE lower(email) = lower(");
        sql.bind(address.as_str()).push(")");
        scope.execute(sql).await?;
    }
    Ok(())
}
