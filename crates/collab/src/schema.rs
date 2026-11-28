//! What the editor's schema says that the shared content alone does not: the attributes
//! each kind of node and mark has, the value an editor gives one that is left out, and
//! the order in which it stores marks; and which of those values the stored form
//! leaves out.
//!
//! The web app writes its own schema into `contracts/ydoc-vectors.json`; a test holds
//! these tables to it.

/// The value an editor gives an attribute that is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fallback {
    /// None: the attribute stays left out.
    None,
    /// A whole number.
    Integer(i64),
    /// A truth value.
    Boolean(bool),
    /// A string.
    Text(&'static str),
}

/// Whether the stored form writes an attribute whose value is its [`Fallback`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtFallback {
    /// Written: a body has to have the attribute.
    Written,
    /// Left out, as a body may leave it: what is stored is then no larger than the body
    /// the document was made of, which may have left it out at the limit of size.
    LeftOut,
}

/// An attribute a kind of node has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Attribute {
    /// Its name.
    pub(crate) name: &'static str,
    /// What an editor reads when the shared content does not have it.
    pub(crate) fallback: Fallback,
    /// Whether the stored form writes it when its value is the fallback.
    pub(crate) at_fallback: AtFallback,
}

const fn plain(name: &'static str) -> Attribute {
    Attribute {
        name,
        fallback: Fallback::None,
        at_fallback: AtFallback::Written,
    }
}

/// A span of a cell of a table: one, which the stored form leaves out.
const fn span(name: &'static str) -> Attribute {
    Attribute {
        name,
        fallback: Fallback::Integer(1),
        at_fallback: AtFallback::LeftOut,
    }
}

/// The spans of a cell, across columns and down rows.
const CELL: &[Attribute] = &[span("colspan"), span("rowspan")];

/// The tone of a callout: a note where it is left out, which a body has to write.
const TONE: Attribute = Attribute {
    name: "tone",
    fallback: Fallback::Text("info"),
    at_fallback: AtFallback::Written,
};

/// Every kind of node the editor's schema has, with its attributes in the schema's order.
pub(crate) const NODES: [(&str, &[Attribute]); 28] = [
    ("blockMath", &[plain("latex")]),
    ("blockquote", &[]),
    ("boardView", &[plain("viewId")]),
    ("bulletList", &[]),
    ("callout", &[TONE]),
    ("codeBlock", &[plain("language")]),
    ("details", &[]),
    ("detailsContent", &[]),
    ("detailsSummary", &[]),
    ("doc", &[]),
    ("file", &[plain("fileId")]),
    ("hardBreak", &[]),
    (
        "heading",
        &[Attribute {
            name: "level",
            fallback: Fallback::Integer(1),
            at_fallback: AtFallback::Written,
        }],
    ),
    ("horizontalRule", &[]),
    ("image", &[plain("fileId"), plain("alt")]),
    ("inlineMath", &[plain("latex")]),
    ("listItem", &[]),
    ("mention", &[plain("userId")]),
    (
        "orderedList",
        &[Attribute {
            name: "start",
            fallback: Fallback::Integer(1),
            at_fallback: AtFallback::LeftOut,
        }],
    ),
    ("paragraph", &[]),
    ("reference", &[plain("id")]),
    ("table", &[]),
    ("tableCell", CELL),
    ("tableHeader", CELL),
    ("tableRow", &[]),
    (
        "taskItem",
        &[Attribute {
            name: "checked",
            fallback: Fallback::Boolean(false),
            at_fallback: AtFallback::Written,
        }],
    ),
    ("taskList", &[]),
    ("text", &[]),
];

/// The marks in the order the editor stores them on a text, each with its attributes.
pub(crate) const MARKS: [(&str, &[&str]); 5] = [
    ("link", &["href"]),
    ("bold", &[]),
    ("italic", &[]),
    ("strike", &[]),
    ("code", &[]),
];

/// The attributes of a kind of node; `None` for a kind the schema does not have.
pub(crate) fn node_attributes(kind: &str) -> Option<&'static [Attribute]> {
    NODES
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, attributes)| *attributes)
}

/// Where a mark comes among the marks of a text; `None` for one the schema does not have.
pub(crate) fn mark_rank(mark: &str) -> Option<usize> {
    MARKS.iter().position(|(name, _)| *name == mark)
}

/// The attributes of a mark; none for one the schema does not have.
pub(crate) fn mark_attributes(mark: &str) -> &'static [&'static str] {
    MARKS
        .iter()
        .find(|(name, _)| *name == mark)
        .map_or(&[], |(_, attributes)| *attributes)
}

/// Where a kind of node stands, which decides what is put around one to judge it alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// Nowhere: the document itself.
    Document,
    /// Among the blocks of the document, a quote, a list entry, a cell, a callout or
    /// what a toggle hides (a board view only in the document and a toggle).
    Block,
    /// In a toggle: its summary, or what it hides.
    DetailsPart,
    /// In a line: text, and what stands in a line and is no text.
    Line,
    /// In a bullet list or an ordered list.
    ListEntry,
    /// In a task list.
    TaskEntry,
    /// In a table.
    TableRow,
    /// In a row of a table: a cell or a header.
    TableCell,
}

/// Where a kind of node stands. A kind nobody knows is judged as a block, and fails.
pub(crate) fn place(kind: &str) -> Place {
    match kind {
        "doc" => Place::Document,
        "text" | "hardBreak" | "mention" | "reference" | "inlineMath" => Place::Line,
        "listItem" => Place::ListEntry,
        "taskItem" => Place::TaskEntry,
        "tableRow" => Place::TableRow,
        "tableCell" | "tableHeader" => Place::TableCell,
        "detailsSummary" | "detailsContent" => Place::DetailsPart,
        _ => Place::Block,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Map, Value, json};

    use super::*;

    fn vectors() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/ydoc-vectors.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn fallback(attribute: Attribute) -> Value {
        match attribute.fallback {
            Fallback::None => Value::Null,
            Fallback::Integer(number) => json!(number),
            Fallback::Boolean(truth) => json!(truth),
            Fallback::Text(text) => json!(text),
        }
    }

    /// The tables are the schema the web app wrote down from its editor.
    #[test]
    fn the_tables_are_the_editors_schema() {
        let schema = &vectors()["schema"];

        let marks: Vec<Value> = MARKS
            .iter()
            .map(|(name, attributes)| {
                let attrs: Map<String, Value> = attributes
                    .iter()
                    .map(|attribute| ((*attribute).to_owned(), Value::Null))
                    .collect();
                json!({"name": name, "attrs": attrs})
            })
            .collect();
        assert_eq!(schema["marks"], Value::from(marks), "the marks, in order");

        // Read as sets: whether a JSON object keeps its order is a setting of the reader.
        let nodes = schema["nodes"]
            .as_object()
            .expect("the nodes of the schema");
        let mut theirs: Vec<&str> = nodes.keys().map(String::as_str).collect();
        theirs.sort_unstable();
        let ours: Vec<&str> = NODES.iter().map(|(name, _)| *name).collect();
        assert_eq!(ours, theirs, "the kinds of node, by name");
        for (name, attributes) in NODES {
            let mut theirs: Vec<(&str, Value)> = nodes[name]
                .as_object()
                .expect("the attributes of a node")
                .iter()
                .map(|(attribute, value)| (attribute.as_str(), value.clone()))
                .collect();
            theirs.sort_by_key(|(attribute, _)| *attribute);
            let mut ours: Vec<(&str, Value)> = attributes
                .iter()
                .map(|one| (one.name, fallback(*one)))
                .collect();
            ours.sort_by_key(|(attribute, _)| *attribute);
            assert_eq!(ours, theirs, "the attributes of {name}");
        }
    }

    #[test]
    fn a_kind_nobody_knows_has_no_attributes_and_no_rank() {
        assert_eq!(node_attributes("video"), None);
        assert_eq!(mark_rank("glow"), None);
        assert!(mark_attributes("glow").is_empty(), "no attributes");
        assert_eq!(mark_rank("link"), Some(0));
        assert_eq!(mark_rank("code"), Some(4));
    }
}
