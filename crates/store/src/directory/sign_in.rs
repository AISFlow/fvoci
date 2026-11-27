//! The sign-ins under way through an organization's provider, and what a login page
//! may show of an organization: looked up before a request names its organization.

use fvoci_model::LoginOrganization;

use crate::Store;
use crate::db::Id;
use crate::error::StoreError;
use crate::oidc_flows::{FLOW_COLUMNS, LoginFlow, flow_from_row};

impl Store {
    /// Takes the sign-in under way whose `state` has the digest `state_hash`, if the
    /// browser that comes back with it has the digest `browser_hash` of the one that
    /// started it: it is removed, ended or not, so that it comes back once; whether it
    /// ended (`expires_at`) is the caller's to see. `None` if there is none of that
    /// browser; one of another browser stays. The sign-in names its organization: the
    /// request that comes back from the provider does not.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn take_oidc_login_flow(
        &self,
        state_hash: &str,
        browser_hash: &str,
    ) -> Result<Option<LoginFlow>, StoreError> {
        self.system()
            .with_write_conn(async |scope| {
                let mut sql = scope.sql("DELETE FROM oidc_login_flows WHERE state_hash = ");
                sql.bind(state_hash)
                    .push_bind(" AND browser_hash = ", browser_hash)
                    .push([" RETURNING ", FLOW_COLUMNS]);
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(None);
                };
                flow_from_row(&row).map(Some)
            })
            .await
    }

    /// The organization of the sign-in under way whose `state` has the digest
    /// `state_hash`, if there is one: for the login page a refused return goes back to.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn oidc_login_flow_org(
        &self,
        state_hash: &str,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql("SELECT org_id FROM oidc_login_flows WHERE state_hash = ");
                sql.bind(state_hash);
                scope
                    .fetch_optional(sql)
                    .await?
                    .map(|row| row.id(0))
                    .transpose()
            })
            .await
    }

    /// Removes the sign-ins under way, of every organization, that ended before `now`,
    /// and says how many there were; and with them the sign-ins held that ended.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn delete_ended_oidc_login_flows(&self, now: i64) -> Result<u64, StoreError> {
        self.system()
            .transaction(async |scope| {
                let mut sql = scope.sql("DELETE FROM oidc_held_sign_ins WHERE expires_at <= ");
                sql.bind(now);
                scope.execute(sql).await?;
                let mut sql = scope.sql("DELETE FROM oidc_login_flows WHERE expires_at <= ");
                sql.bind(now);
                scope.execute(sql).await
            })
            .await
    }

    /// What a login page may show of an organization: the one with the identifier, or
    /// with none the only organization in use if there is one only. `None` if there is
    /// no such organization, it is not in use, or it is the system's. An account's own
    /// space is none of them: it has no login page and is not counted. Its name, and
    /// whether signing in through its provider is on.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn login_organization(
        &self,
        org_id: Option<&str>,
    ) -> Result<Option<LoginOrganization>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql(
                    "SELECT o.id, o.name, EXISTS (SELECT 1 FROM oidc_providers AS p \
                     WHERE p.org_id = o.id AND p.enabled = ",
                );
                sql.bind(true).push(
                    ") FROM organizations AS o WHERE o.deleted_at IS NULL AND o.is_system = false",
                );
                super::of_people(&mut sql, "o.kind");
                if let Some(org_id) = org_id {
                    sql.push_bind(" AND o.id = ", Id(org_id));
                }
                // Two rows tell that the organization is not the only one.
                sql.push(" ORDER BY o.created_at, o.id LIMIT 2");
                let rows = scope.fetch_all(sql).await?;
                let [row] = rows.as_slice() else {
                    return Ok(None);
                };
                Ok(Some(LoginOrganization {
                    id: row.id(0)?,
                    name: row.text(1)?,
                    oidc: row.bool(2)?,
                }))
            })
            .await
    }
}
