//! A block's change is told of in one place, read of the sources.

use std::collections::BTreeSet;
use std::path::Path;

/// The files under `src/` that name `call` followed by its arguments: where it is
/// defined, and where it is called.
fn files_calling(call: &str) -> BTreeSet<String> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = BTreeSet::new();
    let mut pending = vec![source_root.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("the directory is read") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path).expect("the source is read");
                if source.contains(&format!("{call}(")) {
                    let relative = path.strip_prefix(&source_root).expect("under src");
                    found.insert(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    found
}

#[test]
fn blocks_are_broadcast_and_posted_to_the_webhooks_by_changes_alone() {
    assert_eq!(
        files_calling("broadcast_block_change"),
        BTreeSet::from(["app/changes.rs".to_owned(), "realtime.rs".to_owned()]),
        "a block's change is told by `App::tell` (app/changes.rs)"
    );
    assert_eq!(
        files_calling("tell_webhooks"),
        BTreeSet::from(["app/changes.rs".to_owned(), "app/webhooks.rs".to_owned()]),
        "a block is posted to the webhooks by `App::tell` (app/changes.rs)"
    );
}
