//! Pointing what an archive brought at what came with it: bodies at their newly stored
//! files and at the copies of what they name on other boards.

use std::collections::HashMap;

use fvoci_model::Body;

/// Whether the body shows a file or names a block that came with a copy (`copies`): a
/// body that does neither stays as it was stored.
pub(super) fn came_with_more(body: &Body, copies: &HashMap<String, String>) -> bool {
    let named = body.reference_ids();
    !body.file_ids().is_empty() || named.iter().any(|id| copies.contains_key(*id))
}

/// The body pointed at the files (`file_of` gives the new name of one) and at the
/// copies (`copies`, old identifier to new) that came with it. `None` if nothing is
/// left of it without its files: that is no body.
pub(super) fn pointed(
    body: Body,
    file_of: impl Fn(&str) -> Option<String>,
    copies: &HashMap<String, String>,
) -> Option<Body> {
    let mut body = body.with_files_renamed(file_of)?;
    body.rename_references(|id| copies.get(id).cloned());
    Some(body)
}
