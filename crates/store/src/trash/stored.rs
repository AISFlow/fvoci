//! The JSON forms of a board's sidebar places and file records in `deleted_containers`.

use fvoci_model::FileRecord;

use crate::category_containers::ContainerPlace;

/// A [`ContainerPlace`] as the trash keeps it, in `deleted_containers.places`. Its own type, so
/// that what is stored does not change with the store's interface.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredPlace {
    user_id: String,
    category_id: String,
    sort_order: i64,
    hidden: bool,
}

impl From<ContainerPlace> for StoredPlace {
    fn from(place: ContainerPlace) -> Self {
        Self {
            user_id: place.user_id,
            category_id: place.category_id,
            sort_order: place.sort_order,
            hidden: place.hidden,
        }
    }
}

impl From<StoredPlace> for ContainerPlace {
    fn from(place: StoredPlace) -> Self {
        Self {
            user_id: place.user_id,
            category_id: place.category_id,
            sort_order: place.sort_order,
            hidden: place.hidden,
        }
    }
}

/// The file records of a board as they are kept with the board in the trash. A file
/// record's own JSON form is the one for clients, which leaves out where the file is.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredFiles(Vec<StoredFile>);

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredFile {
    id: String,
    container_id: String,
    stored_name: String,
    name: String,
    extension: String,
    size: i64,
    path: String,
    created_at: i64,
    /// The file's newest version; 1 in what was stored before files had versions.
    #[serde(default = "first_version")]
    version: i64,
    #[serde(default)]
    created_by: String,
}

const fn first_version() -> i64 {
    1
}

impl From<Vec<FileRecord>> for StoredFiles {
    fn from(files: Vec<FileRecord>) -> Self {
        Self(
            files
                .into_iter()
                .map(|file| StoredFile {
                    id: file.id,
                    container_id: file.container_id,
                    stored_name: file.stored_name,
                    name: file.name,
                    extension: file.extension,
                    size: file.size,
                    path: file.path,
                    created_at: file.created_at,
                    version: file.version,
                    created_by: file.created_by,
                })
                .collect(),
        )
    }
}

impl From<StoredFiles> for Vec<FileRecord> {
    fn from(files: StoredFiles) -> Self {
        files
            .0
            .into_iter()
            .map(|file| FileRecord {
                id: file.id,
                container_id: file.container_id,
                stored_name: file.stored_name,
                name: file.name,
                extension: file.extension,
                size: file.size,
                path: file.path,
                created_at: file.created_at,
                version: file.version,
                created_by: file.created_by,
            })
            .collect()
    }
}

pub(crate) async fn undelete_block(
    scope: &mut super::Scope<'_>,
    block_id: &str,
    user_id: &str,
) -> Result<Vec<fvoci_model::Block>, crate::StoreError> {
    use super::{
        Block, Id, StoreError, block_from_json, block_not_in_trash, card_hierarchy, containers,
        parents_first, take_blocks_out_of_trash,
    };
    let mut sql = scope.sql("SELECT container_id FROM deleted_blocks WHERE id = ");
    sql.bind(Id(block_id));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(block_not_in_trash)?;
    if !containers::lock_container(scope, &row.id(0)?).await? {
        return Err(block_not_in_trash());
    }
    let mut sql = scope.sql("SELECT deletion_id FROM deleted_blocks WHERE id = ");
    sql.bind(Id(block_id));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(block_not_in_trash)?;
    let deletion_id = row.id(0)?;

    let mut sql = scope.sql("SELECT block FROM deleted_blocks WHERE deletion_id = ");
    sql.bind(Id(&deletion_id));
    let family: Vec<Block> = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| block_from_json(row, 0))
        .collect::<Result<_, StoreError>>()?;
    let rerooted = card_hierarchy::check_restore(scope, &family).await?;

    let now = scope.now_millis();
    let restored = take_blocks_out_of_trash(scope, &deletion_id, user_id, now, |block| {
        if rerooted.contains(&block.id) {
            block.parent_id.clear();
        }
    })
    .await?;
    if !restored.iter().any(|block| block.id == block_id) {
        return Err(block_not_in_trash());
    }
    let order = parents_first(&restored);
    let mut slots: Vec<Option<Block>> = restored.into_iter().map(Some).collect();
    let ordered: Vec<Block> = order
        .into_iter()
        .filter_map(|index| slots[index].take())
        .collect();
    Ok(ordered)
}

/// Which users still have a row, asked once per user.
#[derive(Debug, Default)]
pub(super) struct KnownUsers(std::collections::HashMap<String, bool>);

impl KnownUsers {
    pub(super) async fn has(
        &mut self,
        scope: &mut super::Scope<'_>,
        user_id: &str,
    ) -> Result<bool, crate::StoreError> {
        if let Some(known) = self.0.get(user_id) {
            return Ok(*known);
        }
        let known = crate::users::user_row_exists(scope, user_id).await?;
        self.0.insert(user_id.to_owned(), known);
        Ok(known)
    }

    /// Clears `user_id` if it names a user whose row is gone: made by nobody we know.
    pub(super) async fn forget_if_gone(
        &mut self,
        scope: &mut super::Scope<'_>,
        user_id: &mut String,
    ) -> Result<(), crate::StoreError> {
        if !user_id.is_empty() && !self.has(scope, user_id).await? {
            user_id.clear();
        }
        Ok(())
    }
}
