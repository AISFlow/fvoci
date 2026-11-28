//! Public links of containers through read tokens.

use fvoci_model::{PublicLink, PublicLinkChange};
use fvoci_store::StoreError;

use super::App;
use crate::auth::secret::new_secret;
use crate::error::{AppError, OrMissing, Reason};

impl App {
    /// Returns the public link state of a container. A container that never had a public
    /// link is disabled.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the state cannot be read.
    pub async fn public_link(&self, container_id: &str) -> Result<PublicLink, AppError> {
        match self.store().get_public_link(container_id).await {
            Ok(link) => Ok(link),
            Err(StoreError::NoRows) => Ok(PublicLink {
                container_id: container_id.to_owned(),
                ..PublicLink::default()
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Changes whether a container has an active public link, on behalf of a user, and
    /// returns the state as stored. A container whose public link is enabled, or
    /// regenerated, gets a read token the server makes; one whose public link is turned
    /// off loses its token. Turning off is never refused; enabling is refused while the
    /// organization has its public links off.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such container, and a 403 with the
    /// reason `public_links_off` for enabling it while the organization's public links
    /// are off.
    pub async fn set_public_link(
        &self,
        container_id: &str,
        change: PublicLinkChange,
        user_id: &str,
    ) -> Result<PublicLink, AppError> {
        if change != PublicLinkChange::Off && !self.public_links_on().await? {
            return Err(
                AppError::forbidden("the organization has turned its public links off")
                    .with_reason(Reason::PublicLinksOff),
            );
        }
        let link = self
            .store()
            .set_public_link(container_id, change, &new_secret(), user_id)
            .await
            .or_missing("board")?;
        // A token that no longer opens the board no longer feeds a connection: whoever
        // followed the board's blocks with the old link is cut off.
        if change != PublicLinkChange::On {
            self.ws().end_readers_of_container(container_id);
        }
        Ok(link)
    }
}
