//! The body of a card or a comment: one document of the web app's editor, stored whole.
//!
//! A body is checked when it is read from a request, before it is stored, against the
//! allowlist of `allowlist.rs`: its nodes and marks, how they nest, its
//! size, how deep it nests and how many nodes it has. A document that is no more than an
//! empty paragraph is no body at all, and is stored as none.

use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::allowlist;

/// The version of the set of nodes and marks a stored body is written in: 4 added the
/// file block, 5 the table.
pub const BODY_VERSION: i64 = 5;

/// The largest body, as serialized JSON, in bytes.
pub const BODY_MAX_BYTES: usize = 8 * 1024 * 1024;

/// How deep a body may nest; the document itself is at depth one.
pub const BODY_MAX_DEPTH: usize = 60;

/// How many nodes a body may have, the document itself included.
pub const BODY_MAX_NODES: usize = 100_000;

/// Why a body is refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BodyError {
    /// The value is no document: not an object of type `doc` with content.
    #[error("the body is no document")]
    NotADocument,
    /// The serialized body is larger than [`BODY_MAX_BYTES`].
    #[error("the body is larger than {BODY_MAX_BYTES} bytes")]
    TooLarge,
    /// The body nests deeper than [`BODY_MAX_DEPTH`].
    #[error("the body nests deeper than {BODY_MAX_DEPTH} levels")]
    TooDeep,
    /// The body has more than [`BODY_MAX_NODES`] nodes.
    #[error("the body has more than {BODY_MAX_NODES} nodes")]
    TooManyNodes,
    /// A table has more rows or columns than a table may
    /// ([`crate::TABLE_MAX_ROWS`], [`crate::TABLE_MAX_COLUMNS`]).
    #[error(
        "the table at {path} has more than {} rows or more than {} columns",
        crate::TABLE_MAX_ROWS,
        crate::TABLE_MAX_COLUMNS
    )]
    TableTooLarge {
        /// Where the table is, as a JSON pointer.
        path: String,
    },
    /// A node is malformed.
    #[error("invalid body node: {problem}")]
    InvalidNode {
        /// Where the node is, as a JSON pointer.
        path: String,
        /// What is wrong with it.
        problem: &'static str,
    },
}

/// How many task items a body has, and how many of them are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskCount {
    /// The task items.
    pub total: u32,
    /// The task items that are checked.
    pub done: u32,
}

/// What a list of cards says about a card's body, without the body: whether there is
/// one and its task items. The store keeps it beside the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BodySummary {
    /// Whether there is a body.
    pub has_body: bool,
    /// The task items of the body.
    pub tasks: TaskCount,
}

/// A body that has been checked. Its JSON form is the document as it was received.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Body {
    document: Value,
}

impl Body {
    /// Reads a body from what a client sent: `null` and an empty document are no body.
    ///
    /// # Errors
    ///
    /// Returns a [`BodyError`] if the value is a document that may not be stored.
    pub fn parse(value: Value) -> Result<Option<Self>, BodyError> {
        if value.is_null() {
            return Ok(None);
        }
        allowlist::check(&value)?;
        if is_empty_document(&value) {
            return Ok(None);
        }
        check_size(&value)?;
        Ok(Some(Self { document: value }))
    }

    /// A body as it was stored, which was checked when it was.
    #[must_use]
    pub fn from_stored(document: Value) -> Self {
        Self { document }
    }

    /// The body as its JSON, which is stored as it is.
    #[must_use]
    pub fn document(&self) -> &Value {
        &self.document
    }

    /// The words of the body without its structure, for searching: a line per paragraph,
    /// heading, code block and formula block. A mention and a reference give nothing.
    #[must_use]
    pub fn text(&self) -> String {
        allowlist::plain_text(&self.document)
    }

    /// What a list says about the body.
    #[must_use]
    pub fn summary(&self) -> BodySummary {
        BodySummary {
            has_body: true,
            tasks: self.tasks(),
        }
    }

    /// Total number of `ProseMirror` nodes in the document, the root `doc` node included.
    #[must_use]
    pub fn node_count(&self) -> usize {
        let mut count: usize = 0;
        visit(&self.document, &mut |_| {
            count = count.saturating_add(1);
        });
        count
    }

    /// Counts the task items of the body.
    #[must_use]
    pub fn tasks(&self) -> TaskCount {
        let mut count = TaskCount::default();
        visit(&self.document, &mut |node| {
            if node.get("type").and_then(Value::as_str) == Some("taskItem") {
                count.total = count.total.saturating_add(1);
                let checked = node
                    .get("attrs")
                    .and_then(|attrs| attrs.get("checked"))
                    .and_then(Value::as_bool);
                if checked == Some(true) {
                    count.done = count.done.saturating_add(1);
                }
            }
        });
        count
    }

    /// The stored names of the files that the images and the file blocks of the body
    /// name, in the order they come, each once. Whatever holds the files of a body (their
    /// references, copies, archives) asks this: both kinds of node live by its rules.
    #[must_use]
    pub fn file_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        visit(&self.document, &mut |node| {
            if let Some(id) = named_file(node, &FILE_NODES)
                && seen.insert(id)
            {
                ids.push(id);
            }
        });
        ids
    }

    /// The identifiers of the blocks the references of the body name, each once, in the
    /// order they come. Whether such a block is there, and for whom, the body does not
    /// say.
    #[must_use]
    pub fn reference_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        visit(&self.document, &mut |node| {
            if let Some(id) = reference_target(node)
                && seen.insert(id)
            {
                ids.push(id);
            }
        });
        ids
    }

    /// The identifiers of the users the mentions of the body name, each once, in the
    /// order they come. Whether such a user is there, and reads the body, it does not
    /// say. A mention emptied for a reader by link ([`Self::clear_mentions`]) names
    /// nobody.
    #[must_use]
    pub fn mention_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        visit(&self.document, &mut |node| {
            if let Some(id) = mentioned_user(node)
                && seen.insert(id)
            {
                ids.push(id);
            }
        });
        ids
    }

    /// The stored name of the file the body's first image shows, in document order
    /// (also inside lists and tables), if it has one. Stops at that image. A file block
    /// is no picture and is passed over.
    #[must_use]
    pub fn first_image_file_id(&self) -> Option<&str> {
        first_image(&self.document)
    }

    /// Empties the `userId` of every mention: the body as it goes to whoever reads with a
    /// read token alone, who is not told who the server's users are. That is a shape that
    /// is sent, never one that is stored or received: the allowlist refuses a mention
    /// without a user.
    pub fn clear_mentions(&mut self) {
        clear_mentions_in(&mut self.document);
    }

    /// Whether the body has a file block, which a comment's body may not have. Stops at
    /// the first.
    #[must_use]
    pub fn has_file_blocks(&self) -> bool {
        any_node(&self.document, &|node| {
            node.get("type").and_then(Value::as_str) == Some(FILE_BLOCK)
        })
    }

    /// Empties the `id` of every reference that names none of `shown`: the body as it
    /// goes to whoever reads a board by its link, who is told of the cards of that board
    /// and of nothing else of the workspace, not even that something is there. Like a
    /// mention without its user, that is a shape that is sent, never one that is stored
    /// or received: the allowlist refuses a reference that names nothing.
    pub fn clear_references_but(&mut self, shown: &HashSet<String>) {
        rename_references_in(&mut self.document, &|id| {
            (!shown.contains(id)).then(String::new)
        });
    }

    /// Puts in every reference what `rename` gives for the identifier it holds, and
    /// leaves a reference it gives nothing for as it is: a copy of a body names the
    /// copies of what was copied with it, and whatever else it named.
    pub fn rename_references(&mut self, rename: impl Fn(&str) -> Option<String>) {
        rename_references_in(&mut self.document, &rename);
    }

    /// The body with its images and file blocks pointed at other files: `rename` gives
    /// the new name of a file, or `None` for a file that has none, whose node becomes an
    /// empty paragraph (a body names no file it does not have, and the place of a block
    /// stays filled). `None` if nothing is left but an empty paragraph: that is no
    /// body. A copy of a body whose files were copied is made so.
    #[must_use]
    pub fn with_files_renamed(mut self, rename: impl Fn(&str) -> Option<String>) -> Option<Self> {
        rename_in(&mut self.document, &rename);
        (!is_empty_document(&self.document)).then_some(self)
    }

    /// Checks that the serialized document still fits within [`BODY_MAX_BYTES`], such as
    /// after [`Body::rename_references`] or [`Body::with_files_renamed`] expanded short
    /// placeholders into full identifiers or stored file names.
    ///
    /// # Errors
    ///
    /// Returns [`BodyError::TooLarge`] if the serialized JSON exceeds [`BODY_MAX_BYTES`].
    pub fn check_size(&self) -> Result<(), BodyError> {
        check_size(&self.document)
    }

    /// Appends the top-level blocks of `extra` after those of `existing` (or returns
    /// `extra` when `existing` is `None`) and checks the combined document against the
    /// body limits ([`BODY_MAX_BYTES`], [`BODY_MAX_DEPTH`], [`BODY_MAX_NODES`]) and
    /// allowlist.
    ///
    /// # Errors
    ///
    /// Returns a [`BodyError`] if either document has no `content` array or the combined
    /// document exceeds the body limits.
    pub fn append_to(existing: Option<&Self>, mut extra: Self) -> Result<Self, BodyError> {
        let Some(existing) = existing else {
            return Ok(extra);
        };
        let mut content = existing
            .document
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(BodyError::NotADocument)?;
        let extra_content = extra
            .document
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .ok_or(BodyError::NotADocument)?;
        content.extend(extra_content);
        let merged = serde_json::json!({ "type": "doc", "content": content });
        Self::parse(merged)?.ok_or(BodyError::NotADocument)
    }
}

/// The kind of node that is a picture of a file.
const IMAGE: &str = "image";

/// The kind of node that is a file to download.
const FILE_BLOCK: &str = "file";

/// The kinds of node that name a file of the board in `fileId`.
const FILE_NODES: [&str; 2] = [IMAGE, FILE_BLOCK];

/// The stored name of the file a node names, if it is of one of `kinds`.
fn named_file<'v>(node: &'v Value, kinds: &[&str]) -> Option<&'v str> {
    let kind = node.get("type").and_then(Value::as_str)?;
    if !kinds.contains(&kind) {
        return None;
    }
    node.get("attrs")?.get("fileId")?.as_str()
}

/// The identifier of the block a reference node names.
fn reference_target(node: &Value) -> Option<&str> {
    if node.get("type").and_then(Value::as_str) != Some("reference") {
        return None;
    }
    node.get("attrs")?.get("id")?.as_str()
}

/// The user a mention node names; `None` for any other node, and for a mention that
/// names nobody.
fn mentioned_user(node: &Value) -> Option<&str> {
    if node.get("type").and_then(Value::as_str) != Some("mention") {
        return None;
    }
    let id = node.get("attrs")?.get("userId")?.as_str()?;
    (!id.is_empty()).then_some(id)
}

/// Empties the `userId` of every mention below and at `node`.
fn clear_mentions_in(node: &mut Value) {
    if node.get("type").and_then(Value::as_str) == Some("mention") {
        if let Some(attrs) = node.get_mut("attrs").and_then(Value::as_object_mut) {
            attrs.insert("userId".to_owned(), Value::String(String::new()));
        }
        return;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            clear_mentions_in(child);
        }
    }
}

/// Puts in every reference below and at `node` what `rename` gives for its identifier,
/// if it gives any.
fn rename_references_in(node: &mut Value, rename: &impl Fn(&str) -> Option<String>) {
    if let Some(new_id) = reference_target(node).and_then(rename) {
        if let Some(attrs) = node.get_mut("attrs").and_then(Value::as_object_mut) {
            attrs.insert("id".to_owned(), Value::String(new_id));
        }
        return;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            rename_references_in(child, rename);
        }
    }
}

/// Renames the file of every image and file block below and at `node`, and turns one
/// whose file has no new name into an empty paragraph.
fn rename_in(node: &mut Value, rename: &impl Fn(&str) -> Option<String>) {
    if let Some(old_name) = named_file(node, &FILE_NODES) {
        match rename(old_name) {
            Some(new_name) => {
                if let Some(attrs) = node.get_mut("attrs").and_then(Value::as_object_mut) {
                    attrs.insert("fileId".to_owned(), Value::String(new_name));
                }
            }
            None => *node = serde_json::json!({"type": "paragraph"}),
        }
        return;
    }
    if let Some(children) = node.get_mut("content").and_then(Value::as_array_mut) {
        for child in children {
            rename_in(child, rename);
        }
    }
}

impl<'de> Deserialize<'de> for Body {
    /// Reads a body and checks it. An empty document is refused here: where a body may
    /// be absent, the member is optional.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match Self::parse(value) {
            Ok(Some(body)) => Ok(body),
            Ok(None) => Err(serde::de::Error::custom(
                "an empty body is sent as null, or left out",
            )),
            Err(error) => Err(serde::de::Error::custom(error)),
        }
    }
}

impl utoipa::PartialSchema for Body {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::Object)
            .description(Some(format!(
                "A document of the editor: a node of type `doc` whose `content` is its \
                 blocks. Which nodes, marks and attributes it may have, and how they \
                 nest, is the allowlist of version {BODY_VERSION}, which the server and \
                 the web app share. At most {} MiB as JSON, {BODY_MAX_DEPTH} levels deep \
                 and {} nodes.",
                BODY_MAX_BYTES / (1024 * 1024),
                in_thousands(BODY_MAX_NODES)
            )))
            .into()
    }
}

/// A number as it is written for a reader: its digits in groups of three, by commas.
fn in_thousands(number: usize) -> String {
    let digits = number.to_string();
    let mut written = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            written.push(',');
        }
        written.push(digit);
    }
    written
}

impl utoipa::ToSchema for Body {}

/// What a patch does to a body. Its JSON form is the member `body` of the patch: left
/// out keeps the body, `null` (or an empty document) takes it away, and a document
/// replaces it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum BodyChange {
    /// The body stays as it is.
    #[default]
    Keep,
    /// The body is taken away.
    Clear,
    /// The body becomes this one.
    Set(Body),
}

impl BodyChange {
    /// Reads what a client sent as the member `body`: `null` and an empty document take
    /// the body away.
    ///
    /// # Errors
    ///
    /// Returns a [`BodyError`] if the value is a document that may not be stored.
    pub fn parse(value: Value) -> Result<Self, BodyError> {
        Ok(Body::parse(value)?.map_or(Self::Clear, Self::Set))
    }

    /// The body the change sets, if it sets one.
    #[must_use]
    pub fn new_body(&self) -> Option<&Body> {
        match self {
            Self::Set(body) => Some(body),
            Self::Keep | Self::Clear => None,
        }
    }

    /// Whether the patch leaves the body as it is.
    #[must_use]
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }

    /// The body after the change, for something that had none.
    #[must_use]
    pub fn into_body(self) -> Option<Body> {
        match self {
            Self::Keep | Self::Clear => None,
            Self::Set(body) => Some(body),
        }
    }

    /// Applies the change to a body.
    pub fn apply_to(&self, body: &mut Option<Body>) {
        match self {
            Self::Keep => {}
            Self::Clear => *body = None,
            Self::Set(new) => *body = Some(new.clone()),
        }
    }
}

impl Serialize for BodyChange {
    /// `null` for [`BodyChange::Clear`] and the document for [`BodyChange::Set`];
    /// [`BodyChange::Keep`] is left out by the member's `skip_serializing_if`, and
    /// written as `null` without it.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(body) => body.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BodyChange {
    /// Reads the member when it is there; a member that is left out is
    /// [`BodyChange::Keep`] by its `default`.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Whether a document the allowlist accepted is no more than one empty paragraph, as
/// the web app's `isEmptyDoc` judges it.
fn is_empty_document(value: &Value) -> bool {
    let Some([only]) = value
        .get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    else {
        return false;
    };
    only.get("type").and_then(Value::as_str) == Some("paragraph")
        && only
            .get("content")
            .is_none_or(|content| content.as_array().is_some_and(Vec::is_empty))
}

/// Counts the bytes written to it, and stops the writing past [`BODY_MAX_BYTES`].
#[derive(Debug, Default)]
struct ByteCount(usize);

impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        if self.0 > BODY_MAX_BYTES {
            return Err(std::io::Error::other("the body is too large"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Checks the size of a document that the allowlist accepted, as JSON without
/// whitespace, without making that JSON.
fn check_size(value: &Value) -> Result<(), BodyError> {
    // Writing a JSON value fails only when the count passes the limit.
    serde_json::to_writer(ByteCount::default(), value).map_err(|_| BodyError::TooLarge)
}

/// The file of the first image at or below `node`, in document order.
fn first_image(node: &Value) -> Option<&str> {
    named_file(node, &[IMAGE]).or_else(|| {
        node.get("content")?
            .as_array()?
            .iter()
            .find_map(first_image)
    })
}

/// Whether `test` holds of `node` or of a node below it; stops at the first that does.
fn any_node(node: &Value, test: &impl Fn(&Value) -> bool) -> bool {
    test(node)
        || node
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(|child| any_node(child, test)))
}

/// Calls `f` for every node of the document, the document first.
fn visit<'v>(node: &'v Value, f: &mut impl FnMut(&'v Value)) {
    f(node);
    if let Some(children) = node.get("content").and_then(Value::as_array) {
        for child in children {
            visit(child, f);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_number_is_written_in_groups_of_three_digits() {
        for (number, written) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1,000"),
            (100_000, "100,000"),
            (1_234_567, "1,234,567"),
        ] {
            assert_eq!(in_thousands(number), written);
        }
    }

    fn paragraph(text: &str) -> Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }

    fn document(blocks: &[Value]) -> Value {
        json!({"type": "doc", "content": blocks})
    }

    #[test]
    fn null_and_an_empty_paragraph_are_no_body() {
        for empty in [
            Value::Null,
            document(&[json!({"type": "paragraph"})]),
            document(&[json!({"type": "paragraph", "content": []})]),
        ] {
            assert_eq!(Body::parse(empty.clone()), Ok(None), "{empty}");
        }
        let body = Body::parse(document(&[paragraph("x")]))
            .expect("a body")
            .expect("not empty");
        assert_eq!(body.document(), &document(&[paragraph("x")]));
    }

    /// An image or a file block whose file has a new name points at it; one whose file
    /// has none becomes an empty paragraph, and the body stays valid.
    #[test]
    fn an_image_or_a_file_block_without_its_file_becomes_an_empty_paragraph() {
        for kind in ["image", "file"] {
            let node = |file: &str| json!({"type": kind, "attrs": {"fileId": file}});
            let body = Body::parse(document(&[
                node("kept.png"),
                json!({"type": "blockquote", "content": [node("lost.png")]}),
                paragraph("x"),
            ]))
            .expect("a body")
            .expect("not empty");
            let body = body
                .with_files_renamed(|old| (old == "kept.png").then(|| "copy.png".to_owned()))
                .expect("still a body");
            let expected = document(&[
                node("copy.png"),
                json!({"type": "blockquote", "content": [{"type": "paragraph"}]}),
                paragraph("x"),
            ]);
            assert_eq!(body.document(), &expected, "{kind}");
            assert!(
                Body::parse(expected).is_ok_and(|body| body.is_some()),
                "still a valid body: {kind}"
            );
            assert_eq!(body.file_ids(), ["copy.png"], "{kind}");
        }
    }

    /// The files of a body are those of its images and of its file blocks, each once,
    /// in document order; only a file block makes a body one a comment may not have.
    #[test]
    fn the_files_of_a_body_are_those_of_its_images_and_file_blocks() {
        let image = |file: &str| json!({"type": "image", "attrs": {"fileId": file}});
        let file = |file: &str| json!({"type": "file", "attrs": {"fileId": file}});
        let body = Body::parse(document(&[
            file("report.pdf"),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                paragraph("item"), image("chart.png"), file("report.pdf"),
            ]}]}),
            file("minutes.hwp"),
        ]))
        .expect("a body")
        .expect("not empty");
        assert_eq!(body.file_ids(), ["report.pdf", "chart.png", "minutes.hwp"]);
        assert!(body.has_file_blocks(), "file blocks");
        assert_eq!(body.text(), "item", "a file block gives the text nothing");
        let pictures = Body::parse(document(&[image("chart.png")]))
            .expect("a body")
            .expect("not empty");
        assert!(!pictures.has_file_blocks(), "an image is no file block");
    }

    /// The users a body mentions are named once each, in the order they come, also
    /// from deep in a list; a mention that was emptied names nobody.
    #[test]
    fn the_mentions_of_a_body_name_each_user_once() {
        let mention = |id: &str| json!({"type": "mention", "attrs": {"userId": id}});
        let (first, second) = (crate::new_id(), crate::new_id());
        let mut body = Body::parse(document(&[
            json!({"type": "paragraph", "content": [mention(&second), mention(&first)]}),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [mention(&second)]},
            ]}]}),
        ]))
        .expect("a body")
        .expect("not empty");
        assert_eq!(body.mention_ids(), [second.as_str(), first.as_str()]);
        body.clear_mentions();
        let nobody: [&str; 0] = [];
        assert_eq!(body.mention_ids(), nobody);
    }

    /// Every mention loses its user, also deep in a list; nothing else changes.
    #[test]
    fn clearing_the_mentions_empties_every_user_id() {
        let mention = |id: &str| json!({"type": "mention", "attrs": {"userId": id}});
        let id = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
        let mut body = Body::parse(document(&[
            json!({"type": "paragraph", "content": [mention(id), {"type": "text", "text": "x"}]}),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [mention(id)]},
            ]}]}),
        ]))
        .expect("a body")
        .expect("not empty");
        body.clear_mentions();
        let value = serde_json::to_value(&body).expect("JSON");
        assert_eq!(value["content"][0]["content"][0], mention(""));
        assert_eq!(value["content"][0]["content"][1]["text"], json!("x"));
        assert_eq!(
            value["content"][1]["content"][0]["content"][0]["content"][0],
            mention("")
        );
    }

    /// Every reference that names none of what is shown loses its identifier, also deep
    /// in a list; one that names what is shown stays, and nothing else changes. And a
    /// reference is renamed where there is a new name for what it names.
    #[test]
    fn references_are_emptied_but_for_what_is_shown_and_renamed_where_there_is_a_name() {
        let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
        let shown = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
        let outside = "01a1c8e0-e250-735f-b851-4aaaa2ad2d84";
        let copy = "01a1c8e0-e250-735f-b851-4aaaa2ad2d85";
        let parsed = Body::parse(document(&[
            json!({"type": "paragraph", "content": [
                reference(shown), {"type": "text", "text": "x"}, reference(outside),
            ]}),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [reference(outside), reference(shown)]},
            ]}]}),
        ]))
        .expect("a body")
        .expect("not empty");

        let mut body = parsed.clone();
        body.clear_references_but(&HashSet::from([shown.to_owned()]));
        let value = serde_json::to_value(&body).expect("JSON");
        let line = &value["content"][0]["content"];
        assert_eq!(line[0], reference(shown));
        assert_eq!(line[1]["text"], json!("x"));
        assert_eq!(line[2], reference(""));
        let item = &value["content"][1]["content"][0]["content"][0]["content"];
        assert_eq!((&item[0], &item[1]), (&reference(""), &reference(shown)));
        assert!(!value.to_string().contains(outside));

        let mut body = parsed;
        body.rename_references(|id| (id == shown).then(|| copy.to_owned()));
        assert_eq!(body.reference_ids(), [copy, outside]);
        let value = serde_json::to_value(&body).expect("JSON");
        assert_eq!(value["content"][0]["content"][1]["text"], json!("x"));
    }

    /// A body that was an image or a file block without its file and nothing else is no
    /// body.
    #[test]
    fn a_body_of_a_lost_file_alone_is_no_body() {
        for kind in ["image", "file"] {
            let body = Body::parse(document(&[
                json!({"type": kind, "attrs": {"fileId": "lost.png"}}),
            ]))
            .expect("a body")
            .expect("not empty");
            assert_eq!(body.with_files_renamed(|_| None), None, "{kind}");
        }
    }

    /// The references of a body are the blocks it names, each once, wherever they stand.
    #[test]
    fn the_references_of_a_body_are_named_once_in_order() {
        let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
        let first = "01a1c8e0-e250-735f-b851-4aaaa2ad2d83";
        let second = "01a1c8e0-e250-735f-b851-4aaaa2ad2d84";
        let body = Body::parse(document(&[
            json!({"type": "paragraph", "content": [reference(second), {"type": "text", "text": "x"}]}),
            json!({"type": "bulletList", "content": [{"type": "listItem", "content": [
                {"type": "paragraph", "content": [reference(first), reference(second)]},
            ]}]}),
        ]))
        .expect("a body")
        .expect("not empty");
        assert_eq!(body.reference_ids(), [second, first]);
        assert_eq!(body.text(), "x", "a reference gives the text nothing");
        let none = Body::from_stored(document(&[paragraph("x")]));
        assert!(none.reference_ids().is_empty(), "no reference");
    }

    #[test]
    fn what_is_no_document_is_refused() {
        for value in [
            json!("text"),
            json!([]),
            json!({"type": "paragraph", "content": [paragraph("x")]}),
            json!({"type": "doc"}),
            json!({"type": "doc", "content": []}),
        ] {
            assert_eq!(
                Body::parse(value.clone()),
                Err(BodyError::NotADocument),
                "{value}"
            );
        }
    }

    #[test]
    fn malformed_nodes_are_refused() {
        for node in [
            json!({"type": "paragraph", "extra": 1}),
            json!({"type": "paragraph", "attrs": null}),
            json!({"content": []}),
            json!({"type": "text", "text": ""}),
            json!({"type": "text", "text": 1}),
            json!({"type": "paragraph", "marks": [1]}),
            json!({"type": "paragraph", "content": {}}),
            json!(7),
        ] {
            let value = document(&[paragraph("x"), node.clone()]);
            assert!(
                matches!(Body::parse(value), Err(BodyError::InvalidNode { .. })),
                "{node}"
            );
        }
    }

    /// A document nested `depth` levels deep, the document counted.
    fn nested(depth: usize) -> Value {
        let mut node = json!({"type": "paragraph"});
        for _ in 2..depth {
            node = json!({"type": "blockquote", "content": [node]});
        }
        document(&[node])
    }

    #[test]
    fn the_depth_is_limited() {
        assert!(
            Body::parse(nested(BODY_MAX_DEPTH)).is_ok(),
            "{BODY_MAX_DEPTH} levels"
        );
        assert_eq!(
            Body::parse(nested(BODY_MAX_DEPTH + 1)),
            Err(BodyError::TooDeep)
        );
    }

    #[test]
    fn the_nodes_are_limited() {
        // The document and its paragraphs.
        let fits = document(&vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES - 1]);
        assert!(Body::parse(fits).is_ok(), "{BODY_MAX_NODES} nodes");
        let over = document(&vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES]);
        assert_eq!(Body::parse(over), Err(BodyError::TooManyNodes));
    }

    #[test]
    fn the_size_is_limited() {
        // The JSON of the document around the text of its one paragraph.
        let frame = serde_json::to_vec(&document(&[paragraph("")]))
            .expect("JSON")
            .len();
        let fits = "x".repeat(BODY_MAX_BYTES - frame);
        assert!(
            Body::parse(document(&[paragraph(&fits)])).is_ok(),
            "{BODY_MAX_BYTES} bytes"
        );
        let over = "x".repeat(BODY_MAX_BYTES - frame + 1);
        assert_eq!(
            Body::parse(document(&[paragraph(&over)])),
            Err(BodyError::TooLarge)
        );
    }

    #[test]
    fn task_items_are_counted() {
        let item = |checked: bool| json!({"type": "taskItem", "attrs": {"checked": checked}, "content": [paragraph("t")]});
        let body = Body::parse(document(&[
            json!({"type": "taskList", "content": [item(true), item(false), item(true)]}),
        ]))
        .expect("a body")
        .expect("not empty");
        assert_eq!(body.tasks(), TaskCount { total: 3, done: 2 });
    }

    /// The first image, wherever it is; a file block before it is no picture.
    #[test]
    fn the_first_image_is_the_first_in_document_order() {
        let image = |file: &str| json!({"type": "image", "attrs": {"fileId": file}});
        let body = Body::from_stored(json!({"type": "doc", "content": [
            paragraph("before"),
            {"type": "file", "attrs": {"fileId": "report.pdf"}},
            {"type": "bulletList", "content": [
                {"type": "listItem", "content": [paragraph("item"), image("in-a-list.png")]},
            ]},
            image("after.png"),
        ]}));
        assert_eq!(body.first_image_file_id(), Some("in-a-list.png"));
        let none = Body::from_stored(json!({"type": "doc", "content": [paragraph("text")]}));
        assert_eq!(none.first_image_file_id(), None);
    }

    #[test]
    fn appending_bodies_merges_blocks_and_enforces_size_and_node_limits() {
        let first = Body::parse(document(&[paragraph("one")]))
            .expect("valid")
            .expect("non-empty");
        let second = Body::parse(document(&[paragraph("two")]))
            .expect("valid")
            .expect("non-empty");

        assert_eq!(
            Body::append_to(None, second.clone()).expect("append to empty"),
            second
        );
        let merged = Body::append_to(Some(&first), second).expect("merged");
        assert_eq!(
            merged.document(),
            &document(&[paragraph("one"), paragraph("two")])
        );

        // Exceeding BODY_MAX_NODES across existing + extra is refused.
        let half = BODY_MAX_NODES / 2;
        let left = Body::parse(document(&vec![json!({"type": "horizontalRule"}); half]))
            .expect("valid")
            .expect("non-empty");
        let right = Body::parse(document(&vec![json!({"type": "horizontalRule"}); half]))
            .expect("valid")
            .expect("non-empty");
        assert_eq!(
            Body::append_to(Some(&left), right),
            Err(BodyError::TooManyNodes)
        );

        // Exceeding BODY_MAX_BYTES across existing + extra is refused.
        let big = Body::parse(document(&[paragraph(
            &"a".repeat(BODY_MAX_BYTES / 2 + 1024),
        )]))
        .expect("valid")
        .expect("non-empty");
        assert_eq!(
            Body::append_to(Some(&big), big.clone()),
            Err(BodyError::TooLarge)
        );
    }
}
