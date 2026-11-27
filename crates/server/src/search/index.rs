//! The index the server keeps on the search server: how its text is analysed, and what
//! a document of it holds.
//!
//! Korean is read by nori: a compound is kept whole and in its parts
//! (`decompound_mode` `mixed`), and what carries no meaning by itself (particles,
//! endings) is left out, so "환불규정을" is found by "환불 규정". Every text is indexed a
//! second time as the standard analyser reads it (`.std`), for what nori takes apart:
//! version numbers, words of other scripts. A search asks both.

use serde_json::{Value, json};

/// The version of what the index holds. An index made from another is made again,
/// and every document sent again: raise it with every change of [`definition`], and
/// of which blocks have a document (2: the pages of wikis beside the cards; 3: the
/// highlighter reads a whole body; 4: no card that is a template of its board; 5:
/// documents are written with versions of their own, kept in `sent`, and removals are
/// remembered longer; 6: the highlighter reads a body of 4 MiB; 7: a card's number on
/// its board, `number`; 8: the highlighter reads a body of 8 MiB).
pub(crate) const VERSION: i64 = 8;

/// How long the search server remembers the version of a document it removed: a put
/// of a lower version that lands within that time is refused, and one that lands later
/// would put the document back. Longer than a write may be late by: one that ran out of
/// its thirty seconds and waited in the search server's queue. Both search servers keep
/// sixty seconds by default.
const REMOVALS_KEPT: &str = "10m";

/// How many characters of a text the highlighter reads to find the piece it shows:
/// more than the longest body there is (8 MiB as it is stored). Both search servers
/// read a million by default, and fail the whole search over a longer text.
const HIGHLIGHTED_CHARS: u32 = 10_000_000;

/// The name of the analyser of Korean text.
const KOREAN: &str = "fvoci_korean";

/// The member of the mapping's `_meta` that says the index is this server's, and
/// holds the version it was made from.
const META: &str = "fvoci";

/// A text field: analysed as Korean, and as the standard analyser reads it under
/// `.std`.
fn text_field(copy_to: Option<&str>) -> Value {
    let mut field = json!({
        "type": "text",
        "analyzer": KOREAN,
        "fields": {"std": {"type": "text", "analyzer": "standard"}},
    });
    if let Some(copy_to) = copy_to {
        field["copy_to"] = json!(copy_to);
    }
    field
}

/// The settings and the mapping the index is made with.
pub(crate) fn definition() -> Value {
    json!({
        "settings": {
            "index": {
                "highlight": {"max_analyzed_offset": HIGHLIGHTED_CHARS},
                "gc_deletes": REMOVALS_KEPT,
            },
            "analysis": {
                "tokenizer": {
                    "fvoci_nori": {
                        "type": "nori_tokenizer",
                        "decompound_mode": "mixed",
                        "discard_punctuation": true,
                    },
                },
                "analyzer": {
                    KOREAN: {
                        "type": "custom",
                        "tokenizer": "fvoci_nori",
                        "filter": ["nori_part_of_speech", "nori_readingform", "lowercase"],
                    },
                },
            },
        },
        "mappings": {
            // A document with a member the mapping does not know is refused: nothing
            // is indexed that this file does not say.
            "dynamic": "strict",
            "_meta": {META: {"version": VERSION}},
            "properties": {
                "orgId": {"type": "keyword"},
                "workspaceId": {"type": "keyword"},
                "containerId": {"type": "keyword"},
                "kind": {"type": "keyword"},
                // A card's board-local number (`#12`); absent for pages, entries and units.
                "number": {"type": "long"},
                // Of the title and the text as they were indexed: kept, not searched.
                "digest": {"type": "keyword", "index": false, "doc_values": false},
                // The version the document was written with: its highest is where a
                // server that starts begins its versions.
                "sent": {"type": "long", "index": false},
                "title": text_field(Some("all")),
                "text": text_field(Some("all")),
                // The title and the text together: what a search must match.
                "all": text_field(None),
            },
        },
    })
}

/// The version the index of this description (`GET /{index}`, the value under the
/// index's name) was made from; `None` for an index that is not this server's.
pub(crate) fn version_of(described: &Value) -> Option<i64> {
    described
        .pointer(&format!("/mappings/_meta/{META}/version"))
        .and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_definition_says_its_version_where_it_is_read_back() {
        assert_eq!(version_of(&definition()), Some(VERSION));
        assert_eq!(version_of(&json!({"mappings": {"properties": {}}})), None);
        assert_eq!(
            version_of(&json!({"mappings": {"_meta": {"other": {"version": 1}}}})),
            None
        );
    }

    #[test]
    fn the_highlighter_reads_more_than_the_longest_body() {
        // A body's text has no more characters than the body stored has bytes.
        let highlighted = usize::try_from(HIGHLIGHTED_CHARS).expect("a count of characters");
        assert!(
            highlighted > fvoci_model::BODY_MAX_BYTES,
            "{highlighted} characters read, a body of up to {} bytes",
            fvoci_model::BODY_MAX_BYTES
        );
    }

    #[test]
    fn every_text_is_read_as_korean_and_as_the_standard_analyser_reads_it() {
        let definition = definition();
        for field in ["title", "text", "all"] {
            let field = &definition["mappings"]["properties"][field];
            assert_eq!(field["analyzer"], KOREAN);
            assert_eq!(field["fields"]["std"]["analyzer"], "standard");
        }
        let analysis = &definition["settings"]["analysis"];
        assert_eq!(
            analysis["tokenizer"]["fvoci_nori"]["type"],
            "nori_tokenizer"
        );
        assert_eq!(
            analysis["tokenizer"]["fvoci_nori"]["decompound_mode"],
            "mixed"
        );
    }
}
