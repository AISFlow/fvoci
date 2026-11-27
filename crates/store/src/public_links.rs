//! Public link access to boards through read tokens.
//!
//! A board whose public link is enabled has a read token; whoever has the token reads
//! the board without signing in. The token is the server's: made when the public link is
//! turned on, forgotten when it is turned off, so that a link that was closed does not
//! open again.

use fvoci_model::{PublicLink, PublicLinkChange};

use crate::Store;
use crate::containers;
use crate::db::{Id, OptId, Scope};
use crate::error::StoreError;

impl Store {
    /// Changes whether a board's public link is enabled and returns the state as stored.
    ///
    /// `fresh_token` is the token to expose the board under if it needs a new one: when
    /// its public link was disabled, and with [`PublicLinkChange::OnWithNewToken`]. The
    /// caller makes it, from a source of secrets; the store decides whether it is used.
    ///
    /// Reading the state and writing the new one are one step: of two requests that
    /// enable a board's public link at once, both are answered the same token.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such board.
    pub async fn set_public_link(
        &self,
        container_id: &str,
        change: PublicLinkChange,
        fresh_token: &str,
        user_id: &str,
    ) -> Result<PublicLink, StoreError> {
        self.transaction(async |scope| {
            // Only boards, wikis and courses have public links; diaries never do.
            if !containers::lock_container_of_kind(scope, container_id)
                .await?
                .is_some_and(|kind| fvoci_model::ContainerKinds::LINKED.holds(kind))
            {
                return Err(StoreError::not_found(format_args!(
                    "board ID={container_id}"
                )));
            }
            // The token an enabled public link has, kept unless a new one is asked for.
            let kept = find_public_link(scope, container_id)
                .await?
                .filter(|current| current.enabled && !current.token.is_empty())
                .map(|current| current.token);
            let token = match (change, kept) {
                (PublicLinkChange::Off, _) => String::new(),
                (PublicLinkChange::On, Some(token)) => token,
                (PublicLinkChange::On | PublicLinkChange::OnWithNewToken, _) => {
                    fresh_token.to_owned()
                }
            };
            let public_link = PublicLink {
                container_id: container_id.to_owned(),
                enabled: change != PublicLinkChange::Off,
                token,
                modified_by: user_id.to_owned(),
                updated_at: scope.now_millis(),
            };
            upsert_public_link(scope, &public_link).await?;
            Ok(public_link)
        })
        .await
    }

    /// Returns the public link state of a board.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoRows`] if the board never had a public link.
    pub async fn get_public_link(&self, container_id: &str) -> Result<PublicLink, StoreError> {
        self.with_conn(async |scope| {
            find_public_link(scope, container_id)
                .await?
                .ok_or(StoreError::NoRows)
        })
        .await
    }

    /// The boards of the handle's organization, or of one workspace of it, whose public
    /// link is enabled, by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn public_link_container_ids(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT s.container_id FROM public_links AS s \
                 JOIN containers AS b ON b.id = s.container_id \
                 JOIN workspaces AS w ON w.id = b.workspace_id WHERE s.enabled = ",
            );
            sql.bind(true).push_bind(" AND w.org_id = ", scope.org()?);
            if let Some(workspace_id) = workspace_id {
                sql.push_bind(" AND w.id = ", Id(workspace_id));
            }
            sql.push(" ORDER BY s.container_id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| row.id(0))
                .collect()
        })
        .await
    }
}

async fn upsert_public_link(
    scope: &mut Scope<'_>,
    public_link: &PublicLink,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "INSERT INTO public_links (container_id, enabled, token, modified_by, updated_at) VALUES (",
    );
    sql.bind(Id(&public_link.container_id))
        .push_bind(", ", public_link.enabled);
    // No token is no value: the column is unique among the tokens there are.
    if public_link.token.is_empty() {
        sql.push(", NULL");
    } else {
        sql.push_bind(", ", &public_link.token);
    }
    sql.push_bind(", ", OptId(&public_link.modified_by))
        .push_bind(", ", public_link.updated_at)
        .push(
            ") ON CONFLICT (container_id) DO UPDATE SET enabled = EXCLUDED.enabled, \
             token = EXCLUDED.token, modified_by = EXCLUDED.modified_by, \
             updated_at = EXCLUDED.updated_at",
        );
    scope.execute(sql).await?;
    Ok(())
}

async fn find_public_link(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Option<PublicLink>, StoreError> {
    let mut sql = scope.sql(
        "SELECT container_id, enabled, token, modified_by, updated_at FROM public_links \
         WHERE container_id = ",
    );
    sql.bind(Id(container_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    Ok(Some(PublicLink {
        container_id: row.id(0)?,
        enabled: row.bool(1)?,
        token: row.text(2)?,
        modified_by: row.id(3)?,
        updated_at: row.int(4)?,
    }))
}
