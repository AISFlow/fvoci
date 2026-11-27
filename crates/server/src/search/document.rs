//! A card or a page as the search server holds it, and what tells whether it holds it
//! as it is now.

use std::fmt::Write as _;

use fvoci_model::SearchKind;
use fvoci_store::SearchDocument;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::version::DocumentVersion;

/// How many bytes of the hash a digest keeps: enough that two texts of one card or
/// page never share one by chance.
const DIGEST_BYTES: usize = 16;

/// The digest of a title and a text: what a document is indexed with, so that a
/// search can tell whether what the search server holds of a card or a page is what
/// it is now.
pub(crate) fn digest(title: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    // The length first: no title and text read as another title and text.
    hasher.update(u64::try_from(title.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(title.as_bytes());
    hasher.update(text.as_bytes());
    let mut digest = String::with_capacity(2 * DIGEST_BYTES);
    for byte in hasher.finalize().iter().take(DIGEST_BYTES) {
        // Writing to a string cannot fail.
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

/// The document of a card or a page, written with `version`: what the mapping of the
/// index names, and nothing else.
pub(crate) fn source(document: &SearchDocument, version: DocumentVersion) -> Value {
    let mut doc = json!({
        "sent": version.get(),
        "orgId": document.org_id,
        "workspaceId": document.workspace_id,
        "containerId": document.container_id,
        "kind": document.kind.as_str(),
        "digest": digest(&document.title, &document.text),
        "title": document.title,
        "text": document.text,
    });
    if document.kind == SearchKind::Card
        && let Some(number) = document.number
    {
        doc["number"] = json!(number);
    }
    doc
}

#[cfg(test)]
mod tests {
    use fvoci_model::SearchKind;

    use super::*;
    use crate::search::index;

    #[test]
    fn a_document_has_the_members_the_index_maps_and_no_other() {
        let document = SearchDocument {
            id: "c".to_owned(),
            org_id: "o".to_owned(),
            workspace_id: "w".to_owned(),
            container_id: "b".to_owned(),
            container_kind: fvoci_model::ContainerKind::Board,
            container_title: "Board".to_owned(),
            kind: SearchKind::Card,
            title: "제목".to_owned(),
            text: "본문".to_owned(),
            number: Some(12),
        };
        let version = crate::search::version::Versions::default().next();
        let doc = source(&document, version);
        let definition = index::definition();
        let mapped = definition["mappings"]["properties"]
            .as_object()
            .expect("properties");
        for member in doc.as_object().expect("an object").keys() {
            assert!(mapped.contains_key(member), "{member} is mapped");
        }
        assert_eq!(doc["kind"], "card");
        assert_eq!(doc["containerId"], "b");
        assert_eq!(doc["number"], 12);
        assert_eq!(doc["digest"], digest("제목", "본문"));
        assert_eq!(doc["sent"], version.get());

        let page = SearchDocument {
            kind: SearchKind::Page,
            container_kind: fvoci_model::ContainerKind::Wiki,
            number: Some(12),
            ..document
        };
        assert!(
            source(&page, version).get("number").is_none(),
            "a page carries no card number"
        );
    }

    #[test]
    fn a_digest_tells_a_title_and_a_text_from_any_other() {
        assert_eq!(digest("ab", "c"), digest("ab", "c"));
        assert_ne!(digest("ab", "c"), digest("a", "bc"));
        assert_ne!(digest("ab", "c"), digest("ab", "d"));
        assert_eq!(digest("", "").len(), 32);
    }
}
