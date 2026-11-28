//! The versions kept of an item's body ([`ItemKind`]: a card, a page, a unit, an
//! entry): listing them, reading one, keeping one now and going back to one, the same
//! for every kind. Each kind keeps its own routes, under its own wall, so that no
//! identifier of one kind reaches another's versions.

use super::items::{ItemKind, ItemOf};
use super::{ApiResult, json_response};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// Parses a path version segment into its numeric version, returning 404 when malformed.
pub(super) fn version_number(text: &str) -> Result<i64, AppError> {
    text.parse().map_err(|_| AppError::not_found("version"))
}

/// The versions kept of the item's body, the newest first and without their bodies,
/// for who may read it.
pub(super) async fn body_versions_of<K: ItemKind>(item: ItemOf<K>) -> ApiResult {
    let (_, found) = item
        .reach(ContainerPermission::View, K::READ_DENIED)
        .await?;
    let versions = item.ws.app.body_versions(&found.id).await?;
    Ok(json_response(&versions))
}

/// Keeps the item's body as a version now, by hand, for an editor of it.
pub(super) async fn keep_body_version_of<K: ItemKind>(item: ItemOf<K>) -> ApiResult {
    let (on, found) = item
        .reach(ContainerPermission::ManageCards, K::KEEP_DENIED)
        .await?;
    let kept = item.ws.app.keep_body_version_now(&found.id).await?;
    tracing::debug!(
        kind = K::WHAT,
        container_id = on.container_id(),
        item_id = %found.id,
        user_id = on.user_id(),
        outcome = ?kept.outcome,
        "KeepBodyVersion"
    );
    Ok(json_response(&kept))
}

/// The version of the item's body the route names, with its body, for who may read it.
pub(super) async fn body_version_of<K: ItemKind>(item: ItemOf<K>) -> ApiResult {
    let (_, found) = item
        .reach(ContainerPermission::View, K::READ_DENIED)
        .await?;
    let version = version_number(item.path.version()?)?;
    let found = item.ws.app.body_version(&found.id, version).await?;
    Ok(json_response(&found))
}

/// Writes the body of the version the route names to the item again, for an editor of
/// it, and answers with the item as it is now ([`ItemKind::answer`]).
pub(super) async fn restore_body_version_of<K: ItemKind>(item: ItemOf<K>) -> ApiResult {
    let (on, found) = item
        .reach(ContainerPermission::ManageCards, K::RESTORE_DENIED)
        .await?;
    let version = version_number(item.path.version()?)?;
    let app = &item.ws.app;
    let block = app
        .restore_body_version(&found, version, on.user_id())
        .await?;
    tracing::debug!(
        kind = K::WHAT,
        container_id = on.container_id(),
        item_id = %block.id,
        user_id = on.user_id(),
        "RestoreBodyVersion"
    );
    K::answer(app, block).await
}
