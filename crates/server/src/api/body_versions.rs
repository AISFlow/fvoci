//! Shared helpers for body-version endpoints across cards, wiki pages, diary entries,
//! and course units.
//!
//! Each resource keeps its own route path, OpenAPI schema, and permission entry point so
//! cross-kind IDs cannot cross routes, while delegating version parsing, listing,
//! manual keeping, single-version reads, and restoration here.

use fvoci_model::{Block, KeptBodyVersion};

use super::{ApiResult, json_response};
use crate::app::App;
use crate::error::AppError;

/// Parses a path version segment into its numeric version, returning 404 when malformed.
pub(super) fn version_number(text: &str) -> Result<i64, AppError> {
    text.parse().map_err(|_| AppError::not_found("version"))
}

/// Lists all kept body versions (newest first, without bodies) for an authorized block.
pub(super) async fn list_body_versions(app: &App, block_id: &str) -> ApiResult {
    let versions = app.body_versions(block_id).await?;
    Ok(json_response(&versions))
}

/// Keeps a manual body version now for an authorized block and returns the outcome.
pub(super) async fn keep_body_version(
    app: &App,
    block_id: &str,
) -> Result<KeptBodyVersion, AppError> {
    app.keep_body_version_now(block_id).await
}

/// Reads a single kept body version with its content for an authorized block.
pub(super) async fn get_body_version(app: &App, block_id: &str, version: &str) -> ApiResult {
    let found = app.body_version(block_id, version_number(version)?).await?;
    Ok(json_response(&found))
}

/// Restores a kept body version onto an authorized block on behalf of `user_id`.
pub(super) async fn restore_body_version(
    app: &App,
    block: &Block,
    version: &str,
    user_id: &str,
) -> Result<Block, AppError> {
    app.restore_body_version(block, version_number(version)?, user_id)
        .await
}
