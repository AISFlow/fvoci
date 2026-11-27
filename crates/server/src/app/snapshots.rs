//! Writing pending collaborative bodies before copying or exporting their cards.

use fvoci_model::BlockType;
use fvoci_store::{BlockFilter, Previews};

use super::App;
use crate::error::AppError;

impl App {
    /// Writes the pending body of every block of the board whose body has a room
    /// ([`BlockType::WITH_ROOM`]) before the board's stored blocks are copied. Listing
    /// without bodies avoids reading them once here and again for the copy.
    pub(super) async fn write_container_bodies(&self, container_id: &str) -> Result<(), AppError> {
        let mut with_rooms = Vec::new();
        for with_room in BlockType::WITH_ROOM {
            let listed = self
                .store()
                .get_listed_blocks(
                    &BlockFilter::container(container_id).with_type(with_room),
                    Previews::Without,
                )
                .await?;
            with_rooms.extend(listed.into_iter().map(|listed| listed.block.id));
        }
        self.collab().write_bodies(self, &with_rooms).await
    }
}
