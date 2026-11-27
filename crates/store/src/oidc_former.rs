//! The provider an organization moved away from, kept for a while after its issuer
//! changed: nobody signs in through it, but somebody the provider that took its place
//! vouches for, who has no account yet, may show who they were at it and so carry
//! their account over. One for an organization: the one last moved away from. Past its
//! time it is read as none, and the server's housekeeping removes it. The provider and
//! its sign-ins are in `oidc.rs`.

use std::time::Duration;

use fvoci_model::AuditAction;

use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{OrganizationHeld, Row, Scope};
use crate::error::StoreError;
use crate::oidc::{StoredProvider, hold_provider_of_organization};
use crate::{Store, Tenant};

/// The columns of a former provider, in the order [`former_from_row`] reads them.
const FORMER_COLUMNS: &str = "org_id, issuer, client_id, client_secret, created_at, expires_at";

/// The provider an organization moved away from, as it is kept.
#[derive(Clone, PartialEq, Eq)]
pub struct FormerProvider {
    /// The organization whose provider it was.
    pub org_id: String,
    /// The issuer, exactly as the provider names itself.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The client secret as the server sealed it, for this organization, issuer and
    /// client; empty for none.
    pub sealed_client_secret: String,
    /// When the organization moved away from it, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it is kept no longer, in milliseconds since the Unix epoch.
    pub expires_at: i64,
}

impl std::fmt::Debug for FormerProvider {
    /// Leaves out the sealed secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormerProvider")
            .field("org_id", &self.org_id)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// How a change of the provider's issuer treats the provider before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Keeping<'a> {
    /// How long the provider before is kept; none for no time.
    pub(crate) time: Duration,
    /// Who changes the provider: what becomes of the former one is recorded as
    /// theirs. `None` records nothing.
    pub(crate) actor_id: Option<&'a str>,
}

impl Keeping<'_> {
    /// Keeps nothing and records nothing: for a change that leaves the issuer.
    pub(crate) const NOTHING: Self = Self {
        time: Duration::ZERO,
        actor_id: None,
    };
}

/// What came of ending the keeping of a former provider before its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormerEnd {
    /// It is ended, and that is recorded.
    Ended,
    /// None is kept: its time ran out, or it was ended already.
    NoneKept,
    /// Another one is kept than the one read: the organization moved again since.
    Another,
}

impl Store {
    /// The former provider of the handle's organization, if one is kept at `now`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn oidc_former_provider(
        &self,
        now: i64,
    ) -> Result<Option<FormerProvider>, StoreError> {
        self.with_conn(async |scope| read_former(scope, now).await)
            .await
    }

    /// Ends the keeping of the former provider of the handle's organization before
    /// its time, recorded as `actor_id`'s, if it is still the one read: the one kept
    /// since `kept_since` (its `created_at`). One whose time ran out at `now` is the
    /// housekeeping's to remove and record, and is none here.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn end_oidc_former_provider(
        &self,
        actor_id: &str,
        kept_since: i64,
        now: i64,
    ) -> Result<FormerEnd, StoreError> {
        self.transaction(async |scope| {
            hold_provider_of_organization(scope).await?;
            let Some(former) = read_former(scope, now).await? else {
                return Ok(FormerEnd::NoneKept);
            };
            if former.created_at != kept_since {
                return Ok(FormerEnd::Another);
            }
            end_former(scope, &former, Ended::By(actor_id)).await?;
            Ok(FormerEnd::Ended)
        })
        .await
    }

    /// Removes the former providers whose time ran out by `now`, each recorded in its
    /// organization as the server's own doing, and says how many there were: those of
    /// every organization for the handle with the system's reach, which the
    /// housekeeping holds; those of its own for a handle of one organization, and
    /// none for a handle of none, on either database.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn end_lapsed_oidc_former_providers(&self, now: i64) -> Result<usize, StoreError> {
        if self.tenant() == Tenant::Unset {
            return Ok(0);
        }
        self.transaction(async |scope| {
            let mut sql = scope.sql("DELETE FROM oidc_former_providers WHERE expires_at <= ");
            sql.bind(now);
            if scope.has_org() {
                sql.push_bind(" AND org_id = ", scope.org()?);
            }
            sql.push(" RETURNING org_id, issuer");
            let rows = scope.fetch_all(sql).await?;
            for row in &rows {
                let org_id = row.id(0)?;
                let entry = AuditEntry::by(
                    &AuditActor::Server,
                    "",
                    AuditAction::OidcFormerProviderLapsed,
                    &org_id,
                )
                .with_detail(&row.text(1)?);
                audit::record_entry_in(scope, &org_id, &entry).await?;
            }
            Ok(rows.len())
        })
        .await
    }
}

/// The former provider of the transaction's organization, if one is kept at `now`.
pub(crate) async fn read_former(
    scope: &mut Scope<'_>,
    now: i64,
) -> Result<Option<FormerProvider>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        FORMER_COLUMNS,
        " FROM oidc_former_providers WHERE org_id = ",
    ]);
    sql.bind(scope.org()?).push_bind(" AND expires_at > ", now);
    scope
        .fetch_optional(sql)
        .await?
        .as_ref()
        .map(former_from_row)
        .transpose()
}

/// Follows a change of the issuer of the transaction's organization's provider, from
/// `before`: the former provider kept until now gives way ([`give_way`]), and `before`
/// is kept in its place for `keeping.time`.
pub(crate) async fn keep_after_move<'c>(
    scope: &mut Scope<'c>,
    _held: OrganizationHeld<'c>,
    before: &StoredProvider,
    keeping: Keeping<'_>,
) -> Result<(), StoreError> {
    give_way(scope, keeping.actor_id).await?;
    if keeping.time.is_zero() {
        return Ok(());
    }
    let now = scope.now_millis();
    let kept_for = i64::try_from(keeping.time.as_millis()).unwrap_or(i64::MAX);
    let mut sql = scope.sql([
        "INSERT INTO oidc_former_providers (",
        FORMER_COLUMNS,
        ") VALUES (",
    ]);
    sql.bind(scope.org()?)
        .push_bind(", ", &before.issuer)
        .push_bind(", ", &before.client_id)
        .push_bind(", ", &before.sealed_client_secret)
        .push_bind(", ", now)
        .push_bind(", ", now.saturating_add(kept_for))
        .push(")");
    scope.execute(sql).await?;
    if let Some(actor_id) = keeping.actor_id {
        let entry = AuditEntry::new(
            "",
            actor_id,
            AuditAction::OidcFormerProviderKept,
            &before.org_id,
        )
        .with_detail(&before.issuer);
        audit::record_entry(scope, &entry).await?;
    }
    Ok(())
}

/// Removes the former provider of the transaction's organization, if there is one,
/// its time run out or not: for another move, or the removal of the provider. One
/// whose time had run out is recorded as lapsed, any other as ended by `actor_id`
/// (by nobody, and so not at all, for `None`).
pub(crate) async fn give_way(
    scope: &mut Scope<'_>,
    actor_id: Option<&str>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        FORMER_COLUMNS,
        " FROM oidc_former_providers WHERE org_id = ",
    ]);
    sql.bind(scope.org()?);
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(());
    };
    let former = former_from_row(&row)?;
    let ended = if former.expires_at <= scope.now_millis() {
        Ended::Lapsed
    } else {
        actor_id.map_or(Ended::Unrecorded, Ended::By)
    };
    end_former(scope, &former, ended).await
}

/// How the keeping of a former provider ended.
#[derive(Debug, Clone, Copy)]
enum Ended<'a> {
    /// Before its time, by this person.
    By(&'a str),
    /// Its time ran out: the server's own doing.
    Lapsed,
    /// Before its time, by nobody who is recorded.
    Unrecorded,
}

/// Removes `former`, the transaction's organization's, and records how it ended, if
/// it was still there: one that another transaction removed meanwhile (the
/// housekeeping holds no organization) was recorded by that one.
async fn end_former(
    scope: &mut Scope<'_>,
    former: &FormerProvider,
    ended: Ended<'_>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM oidc_former_providers WHERE org_id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND created_at = ", former.created_at);
    if scope.execute(sql).await? == 0 {
        return Ok(());
    }
    let entry = match ended {
        Ended::By(actor_id) => AuditEntry::new(
            "",
            actor_id,
            AuditAction::OidcFormerProviderEnded,
            &former.org_id,
        ),
        Ended::Lapsed => AuditEntry::by(
            &AuditActor::Server,
            "",
            AuditAction::OidcFormerProviderLapsed,
            &former.org_id,
        ),
        Ended::Unrecorded => return Ok(()),
    };
    audit::record_entry(scope, &entry.with_detail(&former.issuer)).await
}

fn former_from_row(row: &Row) -> Result<FormerProvider, StoreError> {
    Ok(FormerProvider {
        org_id: row.id(0)?,
        issuer: row.text(1)?,
        client_id: row.text(2)?,
        sealed_client_secret: row.text(3)?,
        created_at: row.int(4)?,
        expires_at: row.int(5)?,
    })
}
