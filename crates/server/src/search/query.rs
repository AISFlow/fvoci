//! What the search server is asked: whose documents a request reaches, and the
//! query of a search.
//!
//! [`Scope::clauses`] is the one place that says whose documents a request reaches;
//! every request that reads or removes documents by a query is built here, and none
//! without an organization. What a user typed goes into `match` queries as a JSON
//! string and nowhere else: the search server reads it as words, never as a query of
//! its own language.

use fvoci_model::SearchKind;
use serde_json::{Value, json};

/// The mark the search server puts before a stretch that matched, and the one after
/// it: characters of a private use area, which no markup is made of.
pub(crate) const MATCH_START: char = '\u{e000}';
/// See [`MATCH_START`].
pub(crate) const MATCH_END: char = '\u{e001}';

/// About how many characters of a body a snippet shows.
const SNIPPET_CHARS: u32 = 160;

/// If `text` is a card-number query (`#12`, or `#00012` normalized to `12`), returns that
/// positive card number. Returns `None` if it does not start with `#`, contains any
/// non-ASCII-digit after `#`, is zero, or exceeds `i32::MAX` (matching OpenAPI `int32`).
pub(crate) fn card_number_query(text: &str) -> Option<u32> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i32>()
        .ok()
        .filter(|number| *number > 0)
        .and_then(|number| u32::try_from(number).ok())
}

/// What a search matches documents by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchBy<'a> {
    /// Every word of the text in the title and body.
    Text(&'a str),
    /// Exact match on a card's board-local number (`#12`).
    CardNumber(u32),
}

/// Which containers of a workspace a search reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Containers<'a> {
    /// These: the ones the user reads.
    Listed(&'a [String]),
    /// All of the workspace: for a user who reads more containers than a request
    /// lists. What comes back is checked one by one all the same.
    All,
}

/// Whose documents a request reaches. There is none without an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope<'a> {
    /// The documents of a workspace of an organization, of some of its containers,
    /// and of a kind if one is asked for.
    Workspace {
        org_id: &'a str,
        workspace_id: &'a str,
        containers: Containers<'a>,
        kind: Option<SearchKind>,
    },
    /// Every document of one container of an organization.
    Container {
        org_id: &'a str,
        container_id: &'a str,
    },
}

impl Scope<'_> {
    /// The conditions every document of the scope meets, for the `filter` of a
    /// `bool` query.
    fn clauses(&self) -> Vec<Value> {
        match *self {
            Self::Workspace {
                org_id,
                workspace_id,
                containers,
                kind,
            } => {
                let mut clauses = vec![
                    json!({"term": {"orgId": org_id}}),
                    json!({"term": {"workspaceId": workspace_id}}),
                ];
                if let Containers::Listed(ids) = containers {
                    clauses.push(json!({"terms": {"containerId": ids}}));
                }
                if let Some(kind) = kind {
                    clauses.push(json!({"term": {"kind": kind.as_str()}}));
                }
                clauses
            }
            Self::Container {
                org_id,
                container_id,
            } => vec![
                json!({"term": {"orgId": org_id}}),
                json!({"term": {"containerId": container_id}}),
            ],
        }
    }

    /// The query of every document of the scope: for removing them.
    pub(crate) fn every_document(&self) -> Value {
        json!({"query": {"bool": {"filter": self.clauses()}}})
    }

    /// The request that finds up to `size` documents of the scope by `match_by`, past
    /// the `from` that match best.
    ///
    /// For [`MatchBy::Text`], a document is found if its title and text together hold
    /// every word of `text`, as Korean or as the standard analyser reads them; one whose
    /// title holds them, or that holds them next to one another, comes first. For
    /// [`MatchBy::CardNumber`], non-template cards whose `number` matches exactly are
    /// filtered under the same tenant/workspace/container scope clauses. Of a document
    /// only its identifier, the digest it was indexed with and (for text searches) a
    /// marked piece of its text come back, and no count of what matched.
    pub(crate) fn search(&self, match_by: MatchBy<'_>, from: usize, size: usize) -> Value {
        match match_by {
            MatchBy::CardNumber(number) => {
                let mut filter = self.clauses();
                if !matches!(
                    self,
                    Self::Workspace {
                        kind: Some(SearchKind::Card),
                        ..
                    }
                ) {
                    filter.push(json!({"term": {"kind": SearchKind::Card.as_str()}}));
                }
                filter.push(json!({"term": {"number": number}}));
                json!({
                    "from": from,
                    "size": size,
                    "_source": ["digest"],
                    "track_total_hits": false,
                    "sort": [{"sent": "desc"}, {"containerId": "asc"}],
                    "query": {"bool": {"filter": filter}},
                })
            }
            MatchBy::Text(text) => {
                let every_word =
                    |field: &str| json!({"match": {field: {"query": text, "operator": "and"}}});
                json!({
                    "from": from,
                    "size": size,
                    "_source": ["digest"],
                    "track_total_hits": false,
                    "query": {"bool": {
                        "filter": self.clauses(),
                        "must": [{"bool": {
                            "should": [every_word("all"), every_word("all.std")],
                            "minimum_should_match": 1,
                        }}],
                        "should": [
                            {"match": {"title": {"query": text, "boost": 3}}},
                            {"match": {"title.std": {"query": text, "boost": 3}}},
                            {"match_phrase": {"all": {"query": text, "slop": 1, "boost": 2}}},
                        ],
                    }},
                    "highlight": {
                        "pre_tags": [MATCH_START.to_string()],
                        "post_tags": [MATCH_END.to_string()],
                        // The words were asked of `all`; they are marked in the text.
                        "require_field_match": false,
                        "fields": {"text": {
                            "number_of_fragments": 1,
                            "fragment_size": SNIPPET_CHARS,
                            "no_match_size": SNIPPET_CHARS,
                        }},
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(request: &Value) -> Vec<Value> {
        request["query"]["bool"]["filter"]
            .as_array()
            .expect("a filter")
            .clone()
    }

    #[test]
    fn card_number_query_normalizes_hash_digits_and_leaves_other_queries_as_text() {
        assert_eq!(card_number_query("#12"), Some(12));
        assert_eq!(card_number_query("  #0012  "), Some(12));
        assert_eq!(card_number_query("#01"), Some(1));
        assert_eq!(card_number_query("#2147483647"), Some(2_147_483_647));
        for other in [
            "",
            "#",
            "#0",
            "#000",
            "12",
            "#12a",
            "#12 plan",
            "plan #12",
            "#+12",
            "#-12",
            "##12",
            "# 12",
            "#2147483648",
            "#999999999999",
            "#١٢",
        ] {
            assert_eq!(card_number_query(other), None, "{other:?}");
        }
    }

    #[test]
    fn every_request_is_held_to_its_organization() {
        let containers = ["b1".to_owned(), "b2".to_owned()];
        let workspace = Scope::Workspace {
            org_id: "o1",
            workspace_id: "w1",
            containers: Containers::Listed(&containers),
            kind: Some(SearchKind::Card),
        };
        assert_eq!(
            filter(&workspace.search(MatchBy::Text("환불"), 0, 10)),
            [
                json!({"term": {"orgId": "o1"}}),
                json!({"term": {"workspaceId": "w1"}}),
                json!({"terms": {"containerId": ["b1", "b2"]}}),
                json!({"term": {"kind": "card"}}),
            ]
        );
        let numbered = workspace.search(MatchBy::CardNumber(12), 0, 10);
        assert_eq!(
            filter(&numbered),
            [
                json!({"term": {"orgId": "o1"}}),
                json!({"term": {"workspaceId": "w1"}}),
                json!({"terms": {"containerId": ["b1", "b2"]}}),
                json!({"term": {"kind": "card"}}),
                json!({"term": {"number": 12}}),
            ]
        );
        assert_eq!(
            numbered["sort"],
            json!([{"sent": "desc"}, {"containerId": "asc"}])
        );
        let wide = Scope::Workspace {
            org_id: "o1",
            workspace_id: "w1",
            containers: Containers::All,
            kind: None,
        };
        assert_eq!(
            filter(&wide.search(MatchBy::Text("환불"), 0, 10)),
            [
                json!({"term": {"orgId": "o1"}}),
                json!({"term": {"workspaceId": "w1"}}),
            ]
        );
        assert_eq!(
            filter(&wide.search(MatchBy::CardNumber(12), 0, 10)),
            [
                json!({"term": {"orgId": "o1"}}),
                json!({"term": {"workspaceId": "w1"}}),
                json!({"term": {"kind": "card"}}),
                json!({"term": {"number": 12}}),
            ]
        );
        let container = Scope::Container {
            org_id: "o1",
            container_id: "b1",
        };
        assert_eq!(
            filter(&container.every_document()),
            [
                json!({"term": {"orgId": "o1"}}),
                json!({"term": {"containerId": "b1"}}),
            ]
        );
    }

    #[test]
    fn what_was_typed_is_words_of_a_match_and_nothing_else() {
        let typed = r#"" OR orgId:* } {"query": {"match_all": {}}}"#;
        let scope = Scope::Workspace {
            org_id: "o1",
            workspace_id: "w1",
            containers: Containers::All,
            kind: None,
        };
        let request = scope.search(MatchBy::Text(typed), 20, 5);
        let mut places = 0;
        let mut pending = vec![&request];
        while let Some(value) = pending.pop() {
            match value {
                Value::String(text) if text == typed => places += 1,
                Value::String(text) => assert!(!text.contains("match_all"), "{text}"),
                Value::Array(items) => pending.extend(items),
                Value::Object(members) => {
                    for (name, member) in members {
                        assert!(!name.contains("match_all"), "{name}");
                        pending.push(member);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(
            places, 5,
            "the two it must match and the three it is ranked by"
        );
        assert_eq!(
            (&request["from"], &request["size"]),
            (&json!(20), &json!(5))
        );
        assert_eq!(request["track_total_hits"], false);
    }
}
